const CODEX_METADATA_ENTRY: &str = "ppt/codex-document.json";
const DEFAULT_SLIDE_MASTER_TEXT_STYLES: &str = r#"<p:txStyles>
<p:titleStyle/>
<p:bodyStyle/>
<p:otherStyle/>
</p:txStyles>"#;

fn import_codex_metadata_document(path: &Path) -> Result<Option<PresentationDocument>, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
    let mut entry = match archive.by_name(CODEX_METADATA_ENTRY) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn build_pptx_bytes(document: &PresentationDocument, action: &str) -> Result<Vec<u8>, String> {
    let bytes = document
        .to_ppt_rs()
        .map_err(|error| format!("{action}: {error}"))?
        .build()
        .map_err(|error| format!("{action}: {error}"))?;
    patch_pptx_package(bytes, document).map_err(|error| format!("{action}: {error}"))
}

struct SlideImageAsset {
    xml: String,
    relationship_xml: String,
    media_path: String,
    media_bytes: Vec<u8>,
    extension: String,
}

struct PictureXmlSpec<'a> {
    description: Option<&'a str>,
    lock_aspect_ratio: bool,
    rotation_degrees: Option<i32>,
    flip_horizontal: bool,
    flip_vertical: bool,
    shape_id: usize,
    relationship_id: &'a str,
    frame: Rect,
    crop: Option<ImageCrop>,
}

fn normalized_image_extension(format: &str) -> String {
    match format.to_ascii_lowercase().as_str() {
        "jpeg" => "jpg".to_string(),
        other => other.to_string(),
    }
}

fn image_relationship_xml(relationship_id: &str, target: &str) -> String {
    format!(
        r#"<Relationship Id="{relationship_id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="{}"/>"#,
        ppt_rs::escape_xml(target)
    )
}

fn picture_xml(spec: PictureXmlSpec<'_>) -> String {
    let PictureXmlSpec {
        description,
        lock_aspect_ratio,
        rotation_degrees,
        flip_horizontal,
        flip_vertical,
        shape_id,
        relationship_id,
        frame,
        crop,
    } = spec;
    let blip_fill = if let Some((crop_left, crop_top, crop_right, crop_bottom)) = crop {
        format!(
            r#"<p:blipFill>
<a:blip r:embed="{relationship_id}"/>
<a:srcRect l="{}" t="{}" r="{}" b="{}"/>
<a:stretch>
<a:fillRect/>
</a:stretch>
</p:blipFill>"#,
            (crop_left * 100_000.0).round() as u32,
            (crop_top * 100_000.0).round() as u32,
            (crop_right * 100_000.0).round() as u32,
            (crop_bottom * 100_000.0).round() as u32,
        )
    } else {
        format!(
            r#"<p:blipFill>
<a:blip r:embed="{relationship_id}"/>
<a:stretch>
<a:fillRect/>
</a:stretch>
</p:blipFill>"#
        )
    };
    let descr = description
        .map(|alt| format!(r#" descr="{}""#, ppt_rs::escape_xml(alt)))
        .unwrap_or_default();
    let no_change_aspect = if lock_aspect_ratio { 1 } else { 0 };
    let rotation = rotation_degrees
        .map(|rotation| format!(r#" rot="{}""#, i64::from(rotation) * 60_000))
        .unwrap_or_default();
    let flip_horizontal = if flip_horizontal {
        r#" flipH="1""#
    } else {
        ""
    };
    let flip_vertical = if flip_vertical {
        r#" flipV="1""#
    } else {
        ""
    };
    format!(
        r#"<p:pic>
<p:nvPicPr>
<p:cNvPr id="{shape_id}" name="Picture {shape_id}"{descr}/>
<p:cNvPicPr>
<a:picLocks noChangeAspect="{no_change_aspect}"/>
</p:cNvPicPr>
<p:nvPr/>
</p:nvPicPr>
{blip_fill}
<p:spPr>
<a:xfrm{rotation}{flip_horizontal}{flip_vertical}>
<a:off x="{}" y="{}"/>
<a:ext cx="{}" cy="{}"/>
</a:xfrm>
<a:prstGeom prst="rect">
<a:avLst/>
</a:prstGeom>
</p:spPr>
</p:pic>"#,
        points_to_emu(frame.left),
        points_to_emu(frame.top),
        points_to_emu(frame.width),
        points_to_emu(frame.height),
    )
}

fn image_picture_xml(
    image: &ImageElement,
    shape_id: usize,
    relationship_id: &str,
    frame: Rect,
    crop: Option<ImageCrop>,
) -> String {
    picture_xml(PictureXmlSpec {
        description: image.alt_text.as_deref(),
        lock_aspect_ratio: image.lock_aspect_ratio,
        rotation_degrees: image.rotation_degrees,
        flip_horizontal: image.flip_horizontal,
        flip_vertical: image.flip_vertical,
        shape_id,
        relationship_id,
        frame,
        crop,
    })
}

fn slide_image_assets(
    document: &PresentationDocument,
    slide: &PresentationSlide,
    next_media_index: &mut usize,
) -> Result<Vec<SlideImageAsset>, String> {
    let mut ordered = slide.elements.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|element| element.z_order());
    let shape_count = ordered
        .iter()
        .filter(|element| {
            matches!(
                element,
                PresentationElement::Text(_)
                    | PresentationElement::Shape(_)
                    | PresentationElement::Image(ImageElement { payload: None, .. })
            )
        })
        .count()
        + usize::from(slide.background_fill.is_some());
    let mut image_index = 0_usize;
    let mut assets = Vec::new();
    for element in ordered {
        match element {
            PresentationElement::Image(image) => {
                let Some(payload) = &image.payload else {
                    continue;
                };
                let (left, top, width, height, fitted_crop) = if image.fit_mode != ImageFitMode::Stretch {
                    fit_image(image)
                } else {
                    (
                        image.frame.left,
                        image.frame.top,
                        image.frame.width,
                        image.frame.height,
                        None,
                    )
                };
                image_index += 1;
                let relationship_id = format!("rIdImage{image_index}");
                let extension = normalized_image_extension(&payload.format);
                let media_name = format!("image{next_media_index}.{extension}");
                *next_media_index += 1;
                assets.push(SlideImageAsset {
                    xml: image_picture_xml(
                        image,
                        20 + shape_count + image_index - 1,
                        &relationship_id,
                        Rect {
                            left,
                            top,
                            width,
                            height,
                        },
                        image.crop.or(fitted_crop),
                    ),
                    relationship_xml: image_relationship_xml(
                        &relationship_id,
                        &format!("../media/{media_name}"),
                    ),
                    media_path: format!("ppt/media/{media_name}"),
                    media_bytes: payload.bytes.clone(),
                    extension,
                });
            }
            PresentationElement::Chart(chart) => {
                image_index += 1;
                let relationship_id = format!("rIdImage{image_index}");
                let extension = "png".to_string();
                let media_name = format!("image{next_media_index}.{extension}");
                *next_media_index += 1;
                assets.push(SlideImageAsset {
                    xml: picture_xml(PictureXmlSpec {
                        description: chart.title.as_deref(),
                        lock_aspect_ratio: true,
                        rotation_degrees: None,
                        flip_horizontal: false,
                        flip_vertical: false,
                        shape_id: 20 + shape_count + image_index - 1,
                        relationship_id: &relationship_id,
                        frame: chart.frame,
                        crop: None,
                    }),
                    relationship_xml: image_relationship_xml(
                        &relationship_id,
                        &format!("../media/{media_name}"),
                    ),
                    media_path: format!("ppt/media/{media_name}"),
                    media_bytes: render_chart_png_bytes(document, chart)
                        .map_err(|error| error.to_string())?,
                    extension,
                });
            }
            PresentationElement::Text(_)
            | PresentationElement::Shape(_)
            | PresentationElement::Connector(_)
            | PresentationElement::Table(_) => {}
        }
    }
    Ok(assets)
}

fn patch_pptx_package(
    source_bytes: Vec<u8>,
    document: &PresentationDocument,
) -> Result<Vec<u8>, String> {
    let mut archive =
        ZipArchive::new(Cursor::new(source_bytes)).map_err(|error| error.to_string())?;
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let mut next_media_index = 1_usize;
    let mut pending_slide_relationships = HashMap::new();
    let mut pending_notes_relationships = HashMap::new();
    let mut pending_slide_images = HashMap::new();
    let mut pending_media = Vec::new();
    let mut image_extensions = BTreeSet::new();
    for (slide_index, slide) in document.slides.iter().enumerate() {
        let slide_number = slide_index + 1;
        let images = slide_image_assets(document, slide, &mut next_media_index)?;
        let mut relationships = slide_hyperlink_relationships(slide);
        relationships.extend(images.iter().map(|image| image.relationship_xml.clone()));
        if !relationships.is_empty() {
            pending_slide_relationships.insert(slide_number, relationships);
        }
        let notes_relationships = notes_hyperlink_relationships(slide);
        if !notes_relationships.is_empty() {
            pending_notes_relationships.insert(slide_number, notes_relationships);
        }
        if !images.is_empty() {
            image_extensions.extend(images.iter().map(|image| image.extension.clone()));
            pending_media.extend(
                images
                    .iter()
                    .map(|image| (image.media_path.clone(), image.media_bytes.clone())),
            );
            pending_slide_images.insert(slide_number, images);
        }
    }

    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|error| error.to_string())?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        if name == CODEX_METADATA_ENTRY {
            continue;
        }
        let options = file.options();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        writer
            .start_file(&name, options)
            .map_err(|error| error.to_string())?;
        if name == "[Content_Types].xml" {
            writer
                .write_all(update_content_types_xml(bytes, &image_extensions)?.as_bytes())
                .map_err(|error| error.to_string())?;
            continue;
        }
        if name == "ppt/presentation.xml" {
            writer
                .write_all(
                    update_presentation_xml_dimensions(bytes, document.slide_size)?.as_bytes(),
                )
                .map_err(|error| error.to_string())?;
            continue;
        }
        if name == "ppt/theme/theme1.xml" {
            writer
                .write_all(update_theme_xml(bytes, &document.theme)?.as_bytes())
                .map_err(|error| error.to_string())?;
            continue;
        }
        if name == "ppt/slideMasters/slideMaster1.xml" {
            writer
                .write_all(update_slide_master_xml(bytes)?.as_bytes())
                .map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(slide_number) = parse_slide_xml_path(&name) {
            writer
                .write_all(
                    update_slide_xml(
                        bytes,
                        &document.slides[slide_number - 1],
                        pending_slide_images
                            .get(&slide_number)
                            .map(std::vec::Vec::as_slice)
                            .unwrap_or(&[]),
                    )?
                    .as_bytes(),
                )
                .map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(slide_number) = parse_slide_relationships_path(&name)
            && let Some(relationships) = pending_slide_relationships.remove(&slide_number)
        {
            writer
                .write_all(update_slide_relationships_xml(bytes, &relationships)?.as_bytes())
                .map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(slide_number) = parse_notes_slide_xml_path(&name) {
            writer
                .write_all(
                    update_notes_slide_xml(bytes, &document.slides[slide_number - 1], document)?
                        .as_bytes(),
                )
                .map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(slide_number) = parse_notes_slide_relationships_path(&name)
            && let Some(relationships) = pending_notes_relationships.remove(&slide_number)
        {
            writer
                .write_all(update_slide_relationships_xml(bytes, &relationships)?.as_bytes())
                .map_err(|error| error.to_string())?;
            continue;
        }
        writer
            .write_all(&bytes)
            .map_err(|error| error.to_string())?;
    }

    for (slide_number, relationships) in pending_slide_relationships {
        writer
            .start_file(
                format!("ppt/slides/_rels/slide{slide_number}.xml.rels"),
                SimpleFileOptions::default(),
            )
            .map_err(|error| error.to_string())?;
        writer
            .write_all(slide_relationships_xml(&relationships).as_bytes())
            .map_err(|error| error.to_string())?;
    }

    for (slide_number, relationships) in pending_notes_relationships {
        writer
            .start_file(
                format!("ppt/notesSlides/_rels/notesSlide{slide_number}.xml.rels"),
                SimpleFileOptions::default(),
            )
            .map_err(|error| error.to_string())?;
        writer
            .write_all(slide_relationships_xml(&relationships).as_bytes())
            .map_err(|error| error.to_string())?;
    }

    for (path, bytes) in pending_media {
        writer
            .start_file(path, SimpleFileOptions::default())
            .map_err(|error| error.to_string())?;
        writer
            .write_all(&bytes)
            .map_err(|error| error.to_string())?;
    }

    writer
        .start_file(CODEX_METADATA_ENTRY, SimpleFileOptions::default())
        .map_err(|error| error.to_string())?;
    writer
        .write_all(
            &serde_json::to_vec(document).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;

    writer
        .finish()
        .map_err(|error| error.to_string())
        .map(Cursor::into_inner)
}

fn update_presentation_xml_dimensions(
    existing_bytes: Vec<u8>,
    slide_size: Rect,
) -> Result<String, String> {
    let existing = String::from_utf8(existing_bytes).map_err(|error| error.to_string())?;
    let updated = replace_self_closing_xml_tag(
        &existing,
        "p:sldSz",
        &format!(
            r#"<p:sldSz cx="{}" cy="{}" type="screen4x3"/>"#,
            points_to_emu(slide_size.width),
            points_to_emu(slide_size.height)
        ),
    )?;
    replace_self_closing_xml_tag(
        &updated,
        "p:notesSz",
        &format!(
            r#"<p:notesSz cx="{}" cy="{}"/>"#,
            points_to_emu(slide_size.height),
            points_to_emu(slide_size.width)
        ),
    )
}

fn update_slide_master_xml(existing_bytes: Vec<u8>) -> Result<String, String> {
    let existing = String::from_utf8(existing_bytes).map_err(|error| error.to_string())?;
    if existing.contains("<p:txStyles>") {
        return Ok(existing);
    }

    let closing_tag = "</p:sldMaster>";
    let start = existing
        .find(closing_tag)
        .ok_or_else(|| "slide master xml is missing `</p:sldMaster>`".to_string())?;
    Ok(format!(
        "{}{}{}",
        &existing[..start],
        DEFAULT_SLIDE_MASTER_TEXT_STYLES,
        &existing[start..]
    ))
}

fn replace_self_closing_xml_tag(xml: &str, tag: &str, replacement: &str) -> Result<String, String> {
    let start = xml
        .find(&format!("<{tag} "))
        .ok_or_else(|| format!("presentation xml is missing `<{tag} .../>`"))?;
    let end = xml[start..]
        .find("/>")
        .map(|offset| start + offset + 2)
        .ok_or_else(|| format!("presentation xml tag `{tag}` is not self-closing"))?;
    Ok(format!("{}{replacement}{}", &xml[..start], &xml[end..]))
}

fn update_theme_xml(existing_bytes: Vec<u8>, theme: &ThemeState) -> Result<String, String> {
    let mut existing = String::from_utf8(existing_bytes).map_err(|error| error.to_string())?;

    if let Some(major_font) = theme_font_value(theme, &["major_font"]) {
        existing = replace_theme_font(existing, "majorFont", &major_font)?;
    }
    if let Some(minor_font) = theme_font_value(theme, &["minor_font"]) {
        existing = replace_theme_font(existing, "minorFont", &minor_font)?;
    }

    for (tag, keys) in [
        ("dk1", &["dk1", "tx1", "text1", "dark1"][..]),
        ("lt1", &["lt1", "bg1", "background1", "light1"][..]),
        ("dk2", &["dk2", "tx2", "text2", "dark2"][..]),
        ("lt2", &["lt2", "bg2", "background2", "light2"][..]),
        ("accent1", &["accent1"][..]),
        ("accent2", &["accent2"][..]),
        ("accent3", &["accent3"][..]),
        ("accent4", &["accent4"][..]),
        ("accent5", &["accent5"][..]),
        ("accent6", &["accent6"][..]),
        ("hlink", &["hlink"][..]),
        ("folHlink", &["folhlink"][..]),
    ] {
        if let Some(color) = theme_color_value(theme, keys) {
            existing = replace_theme_color(existing, tag, &color)?;
        }
    }

    Ok(existing)
}

fn theme_font_value(theme: &ThemeState, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| match *key {
        "major_font" => theme.major_font.clone(),
        "minor_font" => theme.minor_font.clone(),
        _ => None,
    })
}

fn theme_color_value(theme: &ThemeState, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        theme.color_scheme.get(*key).map(|value| {
            value
                .trim_start_matches('#')
                .trim()
                .to_ascii_uppercase()
        })
    })
}

fn replace_theme_font(xml: String, tag: &str, font: &str) -> Result<String, String> {
    let start_tag = format!("<a:{tag}>");
    let end_tag = format!("</a:{tag}>");
    let start = xml
        .find(&start_tag)
        .ok_or_else(|| format!("theme xml is missing `<a:{tag}>`"))?;
    let end = xml[start..]
        .find(&end_tag)
        .map(|offset| start + offset + end_tag.len())
        .ok_or_else(|| format!("theme xml is missing `</a:{tag}>`"))?;
    let replacement = format!(
        r#"<a:{tag}>
        <a:latin typeface="{}"/>
        <a:ea typeface=""/>
        <a:cs typeface=""/>
      </a:{tag}>"#,
        ppt_rs::escape_xml(font)
    );
    Ok(format!("{}{replacement}{}", &xml[..start], &xml[end..]))
}

fn replace_theme_color(xml: String, tag: &str, color: &str) -> Result<String, String> {
    let start_tag = format!("<a:{tag}");
    let end_tag = format!("</a:{tag}>");
    let start = xml
        .find(&start_tag)
        .ok_or_else(|| format!("theme xml is missing `<a:{tag}>`"))?;
    let end = xml[start..]
        .find(&end_tag)
        .map(|offset| start + offset + end_tag.len())
        .ok_or_else(|| format!("theme xml is missing `</a:{tag}>`"))?;
    let replacement = format!(r#"<a:{tag}><a:srgbClr val="{color}"/></a:{tag}>"#);
    Ok(format!("{}{replacement}{}", &xml[..start], &xml[end..]))
}

fn slide_hyperlink_relationships(slide: &PresentationSlide) -> Vec<String> {
    let mut ordered = slide.elements.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|element| element.z_order());
    let mut relationship_ids = HashSet::new();
    let mut relationships = Vec::new();
    for element in ordered {
        match element {
            PresentationElement::Text(text) => {
                if let Some(hyperlink) = &text.hyperlink {
                    let relationship_id = format!("rIdHyperlink_{}", text.element_id);
                    if relationship_ids.insert(relationship_id.clone()) {
                        relationships.push(hyperlink.relationship_xml(&relationship_id));
                    }
                }
                collect_rich_text_hyperlinks(
                    &mut relationships,
                    &mut relationship_ids,
                    &text.element_id,
                    &text.rich_text,
                );
            }
            PresentationElement::Shape(shape) => {
                if let Some(hyperlink) = &shape.hyperlink {
                    let relationship_id = format!("rIdHyperlink_{}", shape.element_id);
                    if relationship_ids.insert(relationship_id.clone()) {
                        relationships.push(hyperlink.relationship_xml(&relationship_id));
                    }
                }
                if let Some(rich_text) = &shape.rich_text {
                    collect_rich_text_hyperlinks(
                        &mut relationships,
                        &mut relationship_ids,
                        &shape.element_id,
                        rich_text,
                    );
                }
            }
            PresentationElement::Table(table) => {
                for (row_index, row) in table.rows.iter().enumerate() {
                    for (column_index, cell) in row.iter().enumerate() {
                        collect_rich_text_hyperlinks(
                            &mut relationships,
                            &mut relationship_ids,
                            &table_cell_export_id(&table.element_id, row_index, column_index),
                            &cell.rich_text,
                        );
                    }
                }
            }
            PresentationElement::Connector(_)
            | PresentationElement::Image(_)
            | PresentationElement::Chart(_) => {}
        }
    }
    relationships
}

fn collect_rich_text_hyperlinks(
    relationships: &mut Vec<String>,
    relationship_ids: &mut HashSet<String>,
    element_id: &str,
    rich_text: &RichTextState,
) {
    for range in &rich_text.ranges {
        let Some(hyperlink) = &range.hyperlink else {
            continue;
        };
        let relationship_id = rich_text_relationship_id(element_id, &range.range_id);
        if relationship_ids.insert(relationship_id.clone()) {
            relationships.push(hyperlink.relationship_xml(&relationship_id));
        }
    }
}

fn rich_text_relationship_id(element_id: &str, range_id: &str) -> String {
    let sanitize = |value: &str| {
        value
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect::<String>()
    };
    format!("rIdRange_{}_{}", sanitize(element_id), sanitize(range_id))
}

fn table_cell_export_id(table_id: &str, row_index: usize, column_index: usize) -> String {
    format!("{table_id}_r{row_index}_c{column_index}")
}

fn parse_slide_relationships_path(path: &str) -> Option<usize> {
    path.strip_prefix("ppt/slides/_rels/slide")?
        .strip_suffix(".xml.rels")?
        .parse::<usize>()
        .ok()
}

fn parse_slide_xml_path(path: &str) -> Option<usize> {
    path.strip_prefix("ppt/slides/slide")?
        .strip_suffix(".xml")?
        .parse::<usize>()
        .ok()
}

fn parse_notes_slide_xml_path(path: &str) -> Option<usize> {
    path.strip_prefix("ppt/notesSlides/notesSlide")?
        .strip_suffix(".xml")?
        .parse::<usize>()
        .ok()
}

fn parse_notes_slide_relationships_path(path: &str) -> Option<usize> {
    path.strip_prefix("ppt/notesSlides/_rels/notesSlide")?
        .strip_suffix(".xml.rels")?
        .parse::<usize>()
        .ok()
}

fn update_slide_relationships_xml(
    existing_bytes: Vec<u8>,
    relationships: &[String],
) -> Result<String, String> {
    let existing = String::from_utf8(existing_bytes).map_err(|error| error.to_string())?;
    let injected = relationships.join("\n");
    existing
        .contains("</Relationships>")
        .then(|| existing.replace("</Relationships>", &format!("{injected}\n</Relationships>")))
        .ok_or_else(|| {
            "slide relationships xml is missing a closing `</Relationships>`".to_string()
        })
}

fn slide_relationships_xml(relationships: &[String]) -> String {
    let body = relationships.join("\n");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
{body}
</Relationships>"#
    )
}

fn update_content_types_xml(
    existing_bytes: Vec<u8>,
    image_extensions: &BTreeSet<String>,
) -> Result<String, String> {
    let existing = String::from_utf8(existing_bytes).map_err(|error| error.to_string())?;
    if image_extensions.is_empty() {
        return Ok(existing);
    }
    let existing_lower = existing.to_ascii_lowercase();
    let additions = image_extensions
        .iter()
        .filter(|extension| {
            !existing_lower.contains(&format!(
                r#"extension="{}""#,
                extension.to_ascii_lowercase()
            ))
        })
        .map(|extension| generate_image_content_type(extension))
        .collect::<Vec<_>>();
    if additions.is_empty() {
        return Ok(existing);
    }
    existing
        .contains("</Types>")
        .then(|| existing.replace("</Types>", &format!("{}\n</Types>", additions.join("\n"))))
        .ok_or_else(|| "content types xml is missing a closing `</Types>`".to_string())
}

fn update_slide_xml(
    existing_bytes: Vec<u8>,
    slide: &PresentationSlide,
    slide_images: &[SlideImageAsset],
) -> Result<String, String> {
    let existing = String::from_utf8(existing_bytes).map_err(|error| error.to_string())?;
    let existing = replace_image_placeholders(existing, slide_images)?;
    let existing = apply_shape_block_patches(existing, slide)?;
    let table_xml = slide_table_xml(slide)?;
    if table_xml.is_empty() {
        return Ok(existing);
    }
    existing
        .contains("</p:spTree>")
        .then(|| existing.replace("</p:spTree>", &format!("{table_xml}\n</p:spTree>")))
        .ok_or_else(|| "slide xml is missing a closing `</p:spTree>`".to_string())
}

fn update_notes_slide_xml(
    existing_bytes: Vec<u8>,
    slide: &PresentationSlide,
    document: &PresentationDocument,
) -> Result<String, String> {
    let existing = String::from_utf8(existing_bytes).map_err(|error| error.to_string())?;
    if slide.notes.text.is_empty() {
        return Ok(existing);
    }
    patch_notes_body_text(
        existing,
        &export_notes_text_body_xml(
            &format!("notes_{}", slide.slide_id),
            &slide.notes,
            document,
        ),
    )
}

fn notes_hyperlink_relationships(slide: &PresentationSlide) -> Vec<String> {
    if !slide.notes.visible || slide.notes.text.is_empty() {
        return Vec::new();
    }
    let mut relationship_ids = HashSet::new();
    let mut relationships = Vec::new();
    collect_rich_text_hyperlinks(
        &mut relationships,
        &mut relationship_ids,
        &format!("notes_{}", slide.slide_id),
        &slide.notes.rich_text,
    );
    relationships
}

fn patch_notes_body_text(existing: String, text_body_xml: &str) -> Result<String, String> {
    let placeholder_marker = "<p:ph type=\"body\"";
    let marker = existing
        .find(placeholder_marker)
        .ok_or_else(|| "notes xml is missing the body placeholder".to_string())?;
    let start = existing[..marker]
        .rfind("<p:sp>")
        .ok_or_else(|| "notes xml is missing an opening `<p:sp>` for the body placeholder".to_string())?;
    let end = existing[marker..]
        .find("</p:sp>")
        .map(|offset| marker + offset + "</p:sp>".len())
        .ok_or_else(|| "notes xml is missing a closing `</p:sp>` for the body placeholder".to_string())?;
    let block = &existing[start..end];
    let patched = patch_shape_block_text(block, text_body_xml)?;
    Ok(format!("{}{}{}", &existing[..start], patched, &existing[end..]))
}

fn replace_image_placeholders(
    existing: String,
    slide_images: &[SlideImageAsset],
) -> Result<String, String> {
    if slide_images.is_empty() {
        return Ok(existing);
    }
    let mut updated = String::with_capacity(existing.len());
    let mut remaining = existing.as_str();
    for image in slide_images {
        let marker = remaining
            .find("name=\"Image Placeholder: ")
            .ok_or_else(|| {
                "slide xml is missing an image placeholder block for exported images".to_string()
            })?;
        let start = remaining[..marker].rfind("<p:sp>").ok_or_else(|| {
            "slide xml is missing an opening `<p:sp>` for image placeholder".to_string()
        })?;
        let end = remaining[marker..]
            .find("</p:sp>")
            .map(|offset| marker + offset + "</p:sp>".len())
            .ok_or_else(|| {
                "slide xml is missing a closing `</p:sp>` for image placeholder".to_string()
            })?;
        updated.push_str(&remaining[..start]);
        updated.push_str(&image.xml);
        remaining = &remaining[end..];
    }
    updated.push_str(remaining);
    Ok(updated)
}

#[derive(Clone)]
struct ShapeXmlPatch {
    line_style: Option<LineStyle>,
    flip_horizontal: bool,
    flip_vertical: bool,
    text_body_xml: Option<String>,
}

fn apply_shape_block_patches(
    existing: String,
    slide: &PresentationSlide,
) -> Result<String, String> {
    let mut patches = Vec::new();
    if slide.background_fill.is_some() {
        patches.push(None);
    }
    let mut ordered = slide.elements.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|element| element.z_order());
    for element in ordered {
        match element {
            PresentationElement::Text(text) => patches.push(Some(ShapeXmlPatch {
                line_style: None,
                flip_horizontal: false,
                flip_vertical: false,
                text_body_xml: Some(export_text_body_xml(
                    &text.element_id,
                    &text.text,
                    &text.style,
                    &text.rich_text,
                )),
            })),
            PresentationElement::Shape(shape) => {
                let rich_text = shape.rich_text.clone().unwrap_or_default();
                patches.push(Some(ShapeXmlPatch {
                    line_style: shape
                        .stroke
                        .as_ref()
                        .map(|stroke| stroke.style)
                        .filter(|style| *style != LineStyle::Solid),
                    flip_horizontal: shape.flip_horizontal,
                    flip_vertical: shape.flip_vertical,
                    text_body_xml: shape.text.as_ref().map(|text| {
                        export_text_body_xml(&shape.element_id, text, &shape.text_style, &rich_text)
                    }),
                }));
            }
            PresentationElement::Image(ImageElement { payload: None, .. }) => patches.push(None),
            PresentationElement::Connector(_)
            | PresentationElement::Image(_)
            | PresentationElement::Table(_)
            | PresentationElement::Chart(_) => {}
        }
    }
    if patches.iter().all(|patch| {
        patch.as_ref().is_none_or(|patch| {
            patch.line_style.is_none()
                && !patch.flip_horizontal
                && !patch.flip_vertical
                && patch.text_body_xml.is_none()
        })
    }) {
        return Ok(existing);
    }

    let mut updated = String::with_capacity(existing.len());
    let mut remaining = existing.as_str();
    for patch in patches {
        let Some(start) = remaining.find("<p:sp>") else {
            return Err("slide xml is missing an expected `<p:sp>` block".to_string());
        };
        let end = remaining[start..]
            .find("</p:sp>")
            .map(|offset| start + offset + "</p:sp>".len())
            .ok_or_else(|| "slide xml is missing a closing `</p:sp>` block".to_string())?;
        updated.push_str(&remaining[..start]);
        let block = &remaining[start..end];
        if let Some(patch) = patch {
            updated.push_str(&patch_shape_block(block, patch)?);
        } else {
            updated.push_str(block);
        }
        remaining = &remaining[end..];
    }
    updated.push_str(remaining);
    Ok(updated)
}

fn patch_shape_block(block: &str, patch: ShapeXmlPatch) -> Result<String, String> {
    let block = if let Some(text_body_xml) = patch.text_body_xml {
        patch_shape_block_text(block, &text_body_xml)?
    } else {
        block.to_string()
    };
    let block = if let Some(line_style) = patch.line_style {
        patch_shape_block_dash(&block, line_style)?
    } else {
        block
    };
    if patch.flip_horizontal || patch.flip_vertical {
        patch_shape_block_flip(&block, patch.flip_horizontal, patch.flip_vertical)
    } else {
        Ok(block)
    }
}

fn patch_shape_block_text(block: &str, text_body_xml: &str) -> Result<String, String> {
    if let Some(text_start) = block.find("<p:txBody>") {
        let text_end = block[text_start..]
            .find("</p:txBody>")
            .map(|offset| text_start + offset + "</p:txBody>".len())
            .ok_or_else(|| "shape text body is missing a closing `</p:txBody>`".to_string())?;
        let mut patched = String::with_capacity(block.len() + text_body_xml.len());
        patched.push_str(&block[..text_start]);
        patched.push_str(text_body_xml);
        patched.push_str(&block[text_end..]);
        Ok(patched)
    } else {
        block
            .find("</p:sp>")
            .map(|index| format!("{}{text_body_xml}{}", &block[..index], &block[index..]))
            .ok_or_else(|| "shape block is missing a closing `</p:sp>`".to_string())
    }
}

fn patch_shape_block_dash(block: &str, line_style: LineStyle) -> Result<String, String> {
    let Some(line_start) = block.find("<a:ln") else {
        return Err("shape block is missing an `<a:ln>` entry for stroke styling".to_string());
    };
    if let Some(dash_start) = block[line_start..].find("<a:prstDash") {
        let dash_start = line_start + dash_start;
        let dash_end = block[dash_start..]
            .find("/>")
            .map(|offset| dash_start + offset + 2)
            .ok_or_else(|| "shape line dash entry is missing a closing `/>`".to_string())?;
        let mut patched = String::with_capacity(block.len() + 32);
        patched.push_str(&block[..dash_start]);
        patched.push_str(&format!(
            r#"<a:prstDash val="{}"/>"#,
            line_style.to_ppt_xml()
        ));
        patched.push_str(&block[dash_end..]);
        return Ok(patched);
    }

    if let Some(line_end) = block[line_start..].find("</a:ln>") {
        let line_end = line_start + line_end;
        let mut patched = String::with_capacity(block.len() + 32);
        patched.push_str(&block[..line_end]);
        patched.push_str(&format!(
            r#"<a:prstDash val="{}"/>"#,
            line_style.to_ppt_xml()
        ));
        patched.push_str(&block[line_end..]);
        return Ok(patched);
    }

    let line_end = block[line_start..]
        .find("/>")
        .map(|offset| line_start + offset + 2)
        .ok_or_else(|| "shape line entry is missing a closing marker".to_string())?;
    let line_tag = &block[line_start..line_end - 2];
    let mut patched = String::with_capacity(block.len() + 48);
    patched.push_str(&block[..line_start]);
    patched.push_str(line_tag);
    patched.push('>');
    patched.push_str(&format!(
        r#"<a:prstDash val="{}"/>"#,
        line_style.to_ppt_xml()
    ));
    patched.push_str("</a:ln>");
    patched.push_str(&block[line_end..]);
    Ok(patched)
}

fn patch_shape_block_flip(
    block: &str,
    flip_horizontal: bool,
    flip_vertical: bool,
) -> Result<String, String> {
    let Some(xfrm_start) = block.find("<a:xfrm") else {
        return Err("shape block is missing an `<a:xfrm>` entry for flip styling".to_string());
    };
    let tag_end = block[xfrm_start..]
        .find('>')
        .map(|offset| xfrm_start + offset)
        .ok_or_else(|| "shape transform entry is missing a closing `>`".to_string())?;
    let tag = &block[xfrm_start..=tag_end];
    let mut patched_tag = tag.to_string();
    patched_tag = upsert_xml_attribute(
        &patched_tag,
        "flipH",
        if flip_horizontal { "1" } else { "0" },
    );
    patched_tag =
        upsert_xml_attribute(&patched_tag, "flipV", if flip_vertical { "1" } else { "0" });
    Ok(format!(
        "{}{}{}",
        &block[..xfrm_start],
        patched_tag,
        &block[tag_end + 1..]
    ))
}

fn export_text_body_xml(
    element_id: &str,
    text: &str,
    style: &TextStyle,
    rich_text: &RichTextState,
) -> String {
    let left_inset_attr = concat!("l", "Ins");
    let right_inset_attr = concat!("r", "Ins");
    let top_inset_attr = concat!("t", "Ins");
    let bottom_inset_attr = concat!("b", "Ins");
    let insets = rich_text.layout.insets.unwrap_or(TextInsets {
        left: 6,
        right: 6,
        top: 6,
        bottom: 6,
    });
    let wrap = match rich_text.layout.wrap.unwrap_or(TextWrapMode::Square) {
        TextWrapMode::Square => "square",
        TextWrapMode::None => "none",
    };
    let anchor = match rich_text
        .layout
        .vertical_alignment
        .unwrap_or(TextVerticalAlignment::Top)
    {
        TextVerticalAlignment::Top => "t",
        TextVerticalAlignment::Middle => "ctr",
        TextVerticalAlignment::Bottom => "b",
    };
    let auto_fit = match rich_text.layout.auto_fit.unwrap_or(TextAutoFitMode::None) {
        TextAutoFitMode::None => String::new(),
        TextAutoFitMode::ShrinkText => "<a:normAutofit/>".to_string(),
        TextAutoFitMode::ResizeShapeToFitText => "<a:spAutoFit/>".to_string(),
    };
    let paragraphs = export_text_paragraphs_xml(
        element_id,
        text,
        style,
        style.alignment.unwrap_or(TextAlignment::Left),
        rich_text,
    );
    let left_inset = points_to_emu(insets.left);
    let right_inset = points_to_emu(insets.right);
    let top_inset = points_to_emu(insets.top);
    let bottom_inset = points_to_emu(insets.bottom);
    format!(
        r#"<p:txBody><a:bodyPr wrap="{wrap}" {left_inset_attr}="{left_inset}" {right_inset_attr}="{right_inset}" {top_inset_attr}="{top_inset}" {bottom_inset_attr}="{bottom_inset}" anchor="{anchor}">{auto_fit}</a:bodyPr><a:lstStyle/>{paragraphs}</p:txBody>"#
    )
}

fn export_notes_text_body_xml(
    element_id: &str,
    notes: &NotesState,
    document: &PresentationDocument,
) -> String {
    let style = TextStyle {
        font_size: Some(12),
        font_family: document.theme.minor_font.clone(),
        color: document.theme.resolve_color("tx1"),
        ..TextStyle::default()
    };
    export_text_body_xml(element_id, &notes.text, &style, &notes.rich_text)
}

fn export_table_cell_text_body_xml(
    element_id: &str,
    cell: &TableCellSpec,
) -> String {
    let left_inset_attr = concat!("l", "Ins");
    let right_inset_attr = concat!("r", "Ins");
    let top_inset_attr = concat!("t", "Ins");
    let bottom_inset_attr = concat!("b", "Ins");
    let wrap = match cell.rich_text.layout.wrap.unwrap_or(TextWrapMode::Square) {
        TextWrapMode::Square => "square",
        TextWrapMode::None => "none",
    };
    let anchor = match cell
        .rich_text
        .layout
        .vertical_alignment
        .unwrap_or(TextVerticalAlignment::Top)
    {
        TextVerticalAlignment::Top => "t",
        TextVerticalAlignment::Middle => "ctr",
        TextVerticalAlignment::Bottom => "b",
    };
    let auto_fit = match cell
        .rich_text
        .layout
        .auto_fit
        .unwrap_or(TextAutoFitMode::None)
    {
        TextAutoFitMode::None => String::new(),
        TextAutoFitMode::ShrinkText => "<a:normAutofit/>".to_string(),
        TextAutoFitMode::ResizeShapeToFitText => "<a:spAutoFit/>".to_string(),
    };
    let insets = cell.rich_text.layout.insets.map(|insets| {
        let left_inset = points_to_emu(insets.left);
        let right_inset = points_to_emu(insets.right);
        let top_inset = points_to_emu(insets.top);
        let bottom_inset = points_to_emu(insets.bottom);
        format!(
            r#" {left_inset_attr}="{left_inset}" {right_inset_attr}="{right_inset}" {top_inset_attr}="{top_inset}" {bottom_inset_attr}="{bottom_inset}""#,
        )
    });
    let paragraphs = export_text_paragraphs_xml(
        element_id,
        &cell.text,
        &cell.text_style,
        cell.alignment
            .or(cell.text_style.alignment)
            .unwrap_or(TextAlignment::Left),
        &cell.rich_text,
    );
    format!(
        r#"<a:txBody><a:bodyPr wrap="{wrap}" anchor="{anchor}"{}>{auto_fit}</a:bodyPr><a:lstStyle/>{paragraphs}</a:txBody>"#,
        insets.unwrap_or_default(),
    )
}

fn export_text_paragraphs_xml(
    element_id: &str,
    text: &str,
    style: &TextStyle,
    alignment: TextAlignment,
    rich_text: &RichTextState,
) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut styles = vec![style.clone(); chars.len()];
    let mut hyperlinks = vec![None; chars.len()];
    for range in &rich_text.ranges {
        let start = range.start_cp.min(chars.len());
        let end = range.start_cp.saturating_add(range.length).min(chars.len());
        for entry in styles.iter_mut().skip(start).take(end.saturating_sub(start)) {
            *entry = merged_text_style(entry, &range.style);
        }
        if let Some(hyperlink) = &range.hyperlink {
            let relationship_id = rich_text_relationship_id(element_id, &range.range_id);
            for entry in hyperlinks
                .iter_mut()
                .skip(start)
                .take(end.saturating_sub(start))
            {
                *entry = Some((relationship_id.clone(), hyperlink.clone()));
            }
        }
    }

    let alignment = match alignment {
        TextAlignment::Left => "l",
        TextAlignment::Center => "ctr",
        TextAlignment::Right => "r",
        TextAlignment::Justify => "just",
    };

    let mut paragraphs = Vec::new();
    let mut paragraph_runs: Vec<TextRunExport> = Vec::new();
    let mut current_text = String::new();
    let mut current_style: Option<TextStyle> = None;
    let mut current_hyperlink: Option<(String, HyperlinkState)> = None;
    let mut paragraph_start = 0_usize;

    let flush_run = |paragraph_runs: &mut Vec<TextRunExport>,
                     current_text: &mut String,
                     current_style: &mut Option<TextStyle>,
                     current_hyperlink: &mut Option<(String, HyperlinkState)>| {
        if let Some(style) = current_style.take()
            && !current_text.is_empty()
        {
            paragraph_runs.push(TextRunExport {
                text: std::mem::take(current_text),
                style,
                hyperlink: current_hyperlink.take(),
            });
        }
    };

    for (index, ch) in chars.iter().enumerate() {
        if *ch == '\n' {
            flush_run(
                &mut paragraph_runs,
                &mut current_text,
                &mut current_style,
                &mut current_hyperlink,
            );
            paragraphs.push(render_text_paragraph_xml(
                &paragraph_runs,
                alignment,
                paragraph_spacing_xml(rich_text, paragraph_start, index),
            ));
            paragraph_runs.clear();
            paragraph_start = index + 1;
            continue;
        }

        let next_style = styles[index].clone();
        let next_hyperlink = hyperlinks[index].clone();
        match &current_style {
            Some(existing)
                if text_style_eq(existing, &next_style) && current_hyperlink == next_hyperlink =>
            {
                current_text.push(*ch)
            }
            Some(_) => {
                flush_run(
                    &mut paragraph_runs,
                    &mut current_text,
                    &mut current_style,
                    &mut current_hyperlink,
                );
                current_style = Some(next_style);
                current_hyperlink = next_hyperlink;
                current_text.push(*ch);
            }
            None => {
                current_style = Some(next_style);
                current_hyperlink = next_hyperlink;
                current_text.push(*ch);
            }
        }
    }
    flush_run(
        &mut paragraph_runs,
        &mut current_text,
        &mut current_style,
        &mut current_hyperlink,
    );
    if !paragraph_runs.is_empty() || paragraphs.is_empty() {
        paragraphs.push(render_text_paragraph_xml(
            &paragraph_runs,
            alignment,
            paragraph_spacing_xml(rich_text, paragraph_start, chars.len()),
        ));
    }
    paragraphs.join("")
}

#[derive(Clone)]
struct TextRunExport {
    text: String,
    style: TextStyle,
    hyperlink: Option<(String, HyperlinkState)>,
}

fn render_text_paragraph_xml(
    runs: &[TextRunExport],
    alignment: &str,
    spacing_xml: String,
) -> String {
    if runs.is_empty() {
        return format!(r#"<a:p><a:pPr algn="{alignment}">{spacing_xml}</a:pPr></a:p>"#);
    }
    let runs_xml = runs
        .iter()
        .map(render_text_run_xml)
        .collect::<String>();
    format!(r#"<a:p><a:pPr algn="{alignment}">{spacing_xml}</a:pPr>{runs_xml}</a:p>"#)
}

fn render_text_run_xml(run: &TextRunExport) -> String {
    let TextRunExport {
        text,
        style,
        hyperlink,
    } = run;
    let mut attrs = vec!["lang=\"en-US\"".to_string(), "dirty=\"0\"".to_string()];
    if let Some(font_size) = style.font_size {
        attrs.push(format!(r#"sz="{}""#, font_size * 100));
    }
    if style.bold {
        attrs.push(r#"b="1""#.to_string());
    }
    if style.italic {
        attrs.push(r#"i="1""#.to_string());
    }
    if style.underline {
        attrs.push(r#"u="sng""#.to_string());
    }

    let mut children = String::new();
    if let Some(color) = &style.color {
        children.push_str(&format!(
            r#"<a:solidFill><a:srgbClr val="{}"/></a:solidFill>"#,
            color.trim_start_matches('#').to_ascii_uppercase()
        ));
    }
    if let Some(font_family) = &style.font_family {
        let escaped = ppt_rs::escape_xml(font_family);
        children.push_str(&format!(
            r#"<a:latin typeface="{escaped}"/><a:cs typeface="{escaped}"/>"#
        ));
    }
    if let Some((relationship_id, hyperlink)) = hyperlink {
        let tooltip = hyperlink
            .tooltip
            .as_ref()
            .map(|tooltip| format!(r#" tooltip="{}""#, ppt_rs::escape_xml(tooltip)))
            .unwrap_or_default();
        let highlight = if hyperlink.highlight_click { "1" } else { "0" };
        children.push_str(&format!(
            r#"<a:hlinkClick r:id="{relationship_id}" highlightClick="{highlight}"{tooltip}/>"#
        ));
    }

    format!(
        r#"<a:r><a:rPr {}>{children}</a:rPr><a:t>{}</a:t></a:r>"#,
        attrs.join(" "),
        ppt_rs::escape_xml(text)
    )
}

fn paragraph_spacing_xml(rich_text: &RichTextState, start: usize, end: usize) -> String {
    let mut spacing_before = None;
    let mut spacing_after = None;
    let mut line_spacing = None;
    for range in &rich_text.ranges {
        let range_end = range.start_cp.saturating_add(range.length);
        if range.start_cp >= end || range_end <= start {
            continue;
        }
        if spacing_before.is_none() {
            spacing_before = range.spacing_before;
        }
        if spacing_after.is_none() {
            spacing_after = range.spacing_after;
        }
        if line_spacing.is_none() {
            line_spacing = range.line_spacing;
        }
    }

    let mut xml = String::new();
    if let Some(value) = spacing_before {
        xml.push_str(&format!(r#"<a:spcBef><a:spcPts val="{value}"/></a:spcBef>"#));
    }
    if let Some(value) = spacing_after {
        xml.push_str(&format!(r#"<a:spcAft><a:spcPts val="{value}"/></a:spcAft>"#));
    }
    if let Some(value) = line_spacing {
        let spacing = (value * 100_000.0).round() as i32;
        xml.push_str(&format!(r#"<a:lnSpc><a:spcPct val="{spacing}"/></a:lnSpc>"#));
    }
    xml
}

fn text_style_eq(left: &TextStyle, right: &TextStyle) -> bool {
    left.font_size == right.font_size
        && left.font_family == right.font_family
        && left.color == right.color
        && left.alignment == right.alignment
        && left.bold == right.bold
        && left.italic == right.italic
        && left.underline == right.underline
}

fn upsert_xml_attribute(tag: &str, attribute: &str, value: &str) -> String {
    let needle = format!(r#"{attribute}=""#);
    if let Some(start) = tag.find(&needle) {
        let value_start = start + needle.len();
        if let Some(end_offset) = tag[value_start..].find('"') {
            let end = value_start + end_offset;
            return format!("{}{}{}", &tag[..value_start], value, &tag[end..]);
        }
    }
    let insert_at = tag.len() - 1;
    format!(r#"{} {attribute}="{value}""#, &tag[..insert_at]) + &tag[insert_at..]
}

fn slide_table_xml(slide: &PresentationSlide) -> Result<String, String> {
    let mut ordered = slide.elements.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|element| element.z_order());
    let mut table_index = 0_usize;
    let mut xml = Vec::new();
    for element in ordered {
        let PresentationElement::Table(table) = element else {
            continue;
        };
        table_index += 1;
        let rows = table
            .rows
            .clone()
            .into_iter()
            .enumerate()
            .map(|(row_index, row)| {
                let cells = row
                    .into_iter()
                    .enumerate()
                    .map(|(column_index, cell)| {
                        build_table_cell(cell, &table.merges, row_index, column_index)
                    })
                    .collect::<Vec<_>>();
                let mut table_row = TableRow::new(cells);
                if let Some(height) = table.row_heights.get(row_index) {
                    table_row = table_row.with_height(points_to_emu(*height));
                }
                Some(table_row)
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| "table export failed to build rows".to_string())?;
        let table_xml = ppt_rs::generator::table::generate_table_xml(
            &ppt_rs::generator::table::Table::new(
                rows,
                table
                    .column_widths
                    .iter()
                    .copied()
                    .map(points_to_emu)
                    .collect(),
                points_to_emu(table.frame.left),
                points_to_emu(table.frame.top),
            ),
            300 + table_index,
        );
        xml.push(patch_table_cell_text_bodies(table_xml, table)?);
    }
    Ok(xml.join("\n"))
}

fn patch_table_cell_text_bodies(table_xml: String, table: &TableElement) -> Result<String, String> {
    let mut updated = String::with_capacity(table_xml.len());
    let mut remaining = table_xml.as_str();
    for (row_index, row) in table.rows.iter().enumerate() {
        for (column_index, cell) in row.iter().enumerate() {
            let Some(start) = remaining.find("<a:tc") else {
                return Err("table xml is missing an expected `<a:tc>` block".to_string());
            };
            let end = remaining[start..]
                .find("</a:tc>")
                .map(|offset| start + offset + "</a:tc>".len())
                .ok_or_else(|| "table xml is missing a closing `</a:tc>` block".to_string())?;
            updated.push_str(&remaining[..start]);
            let block = &remaining[start..end];
            if table_cell_is_merged(table, row_index, column_index) {
                updated.push_str(block);
            } else {
                updated.push_str(&patch_table_cell_text(
                    block,
                    &export_table_cell_text_body_xml(
                        &table_cell_export_id(&table.element_id, row_index, column_index),
                        cell,
                    ),
                )?);
            }
            remaining = &remaining[end..];
        }
    }
    updated.push_str(remaining);
    Ok(updated)
}

fn table_cell_is_merged(table: &TableElement, row_index: usize, column_index: usize) -> bool {
    table.merges.iter().any(|merge| {
        row_index >= merge.start_row
            && row_index <= merge.end_row
            && column_index >= merge.start_column
            && column_index <= merge.end_column
            && (row_index != merge.start_row || column_index != merge.start_column)
    })
}

fn patch_table_cell_text(block: &str, text_body_xml: &str) -> Result<String, String> {
    if let Some(text_start) = block.find("<a:txBody>") {
        let text_end = block[text_start..]
            .find("</a:txBody>")
            .map(|offset| text_start + offset + "</a:txBody>".len())
            .ok_or_else(|| "table cell text body is missing a closing `</a:txBody>`".to_string())?;
        let mut patched = String::with_capacity(block.len() + text_body_xml.len());
        patched.push_str(&block[..text_start]);
        patched.push_str(text_body_xml);
        patched.push_str(&block[text_end..]);
        Ok(patched)
    } else {
        Err("table cell block is missing an `<a:txBody>` entry".to_string())
    }
}

pub(crate) fn write_preview_image_bytes(
    png_bytes: &[u8],
    target_path: &Path,
    format: PreviewOutputFormat,
    scale: f32,
    quality: u8,
    action: &str,
) -> Result<(), PresentationArtifactError> {
    if matches!(format, PreviewOutputFormat::Png) && scale == 1.0 {
        std::fs::write(target_path, png_bytes).map_err(|error| {
            PresentationArtifactError::ExportFailed {
                path: target_path.to_path_buf(),
                message: error.to_string(),
            }
        })?;
        return Ok(());
    }
    let mut preview = image::load_from_memory(png_bytes).map_err(|error| {
        PresentationArtifactError::ExportFailed {
            path: target_path.to_path_buf(),
            message: format!("{action}: {error}"),
        }
    })?;
    if scale != 1.0 {
        let width = (preview.width() as f32 * scale).round().max(1.0) as u32;
        let height = (preview.height() as f32 * scale).round().max(1.0) as u32;
        preview = preview.resize_exact(width, height, FilterType::Lanczos3);
    }
    let file = std::fs::File::create(target_path).map_err(|error| {
        PresentationArtifactError::ExportFailed {
            path: target_path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    let mut writer = std::io::BufWriter::new(file);
    match format {
        PreviewOutputFormat::Png => {
            preview
                .write_to(&mut writer, ImageFormat::Png)
                .map_err(|error| PresentationArtifactError::ExportFailed {
                    path: target_path.to_path_buf(),
                    message: format!("{action}: {error}"),
                })?
        }
        PreviewOutputFormat::Jpeg => {
            let rgb = preview.to_rgb8();
            let mut encoder = JpegEncoder::new_with_quality(&mut writer, quality);
            encoder.encode_image(&rgb).map_err(|error| {
                PresentationArtifactError::ExportFailed {
                    path: target_path.to_path_buf(),
                    message: format!("{action}: {error}"),
                }
            })?;
        }
        PreviewOutputFormat::Svg => {
            let mut png_bytes = Cursor::new(Vec::new());
            preview
                .write_to(&mut png_bytes, ImageFormat::Png)
                .map_err(|error| PresentationArtifactError::ExportFailed {
                    path: target_path.to_path_buf(),
                    message: format!("{action}: {error}"),
                })?;
            let embedded_png = BASE64_STANDARD.encode(png_bytes.into_inner());
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}"><image href="data:image/png;base64,{embedded_png}" width="{}" height="{}"/></svg>"#,
                preview.width(),
                preview.height(),
                preview.width(),
                preview.height(),
                preview.width(),
                preview.height(),
            );
            writer.write_all(svg.as_bytes()).map_err(|error| {
                PresentationArtifactError::ExportFailed {
                    path: target_path.to_path_buf(),
                    message: format!("{action}: {error}"),
                }
            })?;
        }
    }
    Ok(())
}

fn parse_preview_output_format(
    format: Option<&str>,
    path: &Path,
    action: &str,
) -> Result<PreviewOutputFormat, PresentationArtifactError> {
    let value = format
        .map(str::to_owned)
        .or_else(|| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "png".to_string());
    match value.to_ascii_lowercase().as_str() {
        "png" => Ok(PreviewOutputFormat::Png),
        "jpg" | "jpeg" => Ok(PreviewOutputFormat::Jpeg),
        "svg" => Ok(PreviewOutputFormat::Svg),
        other => Err(PresentationArtifactError::InvalidArgs {
            action: action.to_string(),
            message: format!("preview format `{other}` is not supported"),
        }),
    }
}

fn normalize_preview_scale(
    scale: Option<f32>,
    action: &str,
) -> Result<f32, PresentationArtifactError> {
    let scale = scale.unwrap_or(1.0);
    if !scale.is_finite() || scale <= 0.0 {
        return Err(PresentationArtifactError::InvalidArgs {
            action: action.to_string(),
            message: "`scale` must be a positive number".to_string(),
        });
    }
    Ok(scale)
}

fn normalize_preview_quality(
    quality: Option<u8>,
    action: &str,
) -> Result<u8, PresentationArtifactError> {
    let quality = quality.unwrap_or(90);
    if quality == 0 || quality > 100 {
        return Err(PresentationArtifactError::InvalidArgs {
            action: action.to_string(),
            message: "`quality` must be between 1 and 100".to_string(),
        });
    }
    Ok(quality)
}
