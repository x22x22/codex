use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use regex::Regex;
use zip::ZipArchive;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::CellAddress;
use crate::CellRange;
use crate::SpreadsheetAlignment;
use crate::SpreadsheetArtifact;
use crate::SpreadsheetArtifactError;
use crate::SpreadsheetBorder;
use crate::SpreadsheetBorderLine;
use crate::SpreadsheetCell;
use crate::SpreadsheetCellFormat;
use crate::SpreadsheetCellValue;
use crate::SpreadsheetFill;
use crate::SpreadsheetFillRectangle;
use crate::SpreadsheetGradientStop;
use crate::SpreadsheetNumberFormat;
use crate::SpreadsheetSheet;
use crate::SpreadsheetTextStyle;

const CODEX_METADATA_PATH: &str = "customXml/item1.xml";

pub(crate) fn write_xlsx(
    artifact: &mut SpreadsheetArtifact,
    path: &Path,
) -> Result<PathBuf, SpreadsheetArtifactError> {
    if artifact.auto_recalculate {
        artifact.recalculate();
    }
    for sheet in &mut artifact.sheets {
        sheet.cleanup_and_validate_sheet()?;
    }

    let file = File::create(path).map_err(|error| SpreadsheetArtifactError::ExportFailed {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();

    let sheet_count = artifact.sheets.len().max(1);
    zip.start_file("[Content_Types].xml", options)
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    zip.write_all(
        content_types_xml(sheet_count, artifact_has_custom_metadata(artifact)).as_bytes(),
    )
    .map_err(|error| SpreadsheetArtifactError::ExportFailed {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;

    zip.add_directory("_rels/", options).map_err(|error| {
        SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    zip.start_file("_rels/.rels", options).map_err(|error| {
        SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    zip.write_all(root_relationships_xml().as_bytes())
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

    zip.add_directory("docProps/", options).map_err(|error| {
        SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    zip.start_file("docProps/app.xml", options)
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    zip.write_all(app_xml(artifact).as_bytes())
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

    zip.start_file("docProps/core.xml", options)
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    zip.write_all(core_xml(artifact).as_bytes())
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

    zip.add_directory("xl/", options)
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    zip.start_file("xl/workbook.xml", options)
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    zip.write_all(workbook_xml(artifact).as_bytes())
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

    zip.add_directory("xl/_rels/", options).map_err(|error| {
        SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    zip.start_file("xl/_rels/workbook.xml.rels", options)
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    zip.write_all(workbook_relationships_xml(artifact).as_bytes())
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

    zip.start_file("xl/styles.xml", options).map_err(|error| {
        SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    zip.write_all(styles_xml(artifact).as_bytes())
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

    if artifact_has_custom_metadata(artifact) {
        zip.add_directory("customXml/", options).map_err(|error| {
            SpreadsheetArtifactError::ExportFailed {
                path: path.to_path_buf(),
                message: error.to_string(),
            }
        })?;
        zip.start_file(CODEX_METADATA_PATH, options)
            .map_err(|error| SpreadsheetArtifactError::ExportFailed {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        zip.write_all(codex_metadata_xml(artifact)?.as_bytes())
            .map_err(|error| SpreadsheetArtifactError::ExportFailed {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
    }

    zip.add_directory("xl/worksheets/", options)
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    if artifact.sheets.is_empty() {
        let empty = SpreadsheetSheet::new("Sheet1".to_string());
        zip.start_file("xl/worksheets/sheet1.xml", options)
            .map_err(|error| SpreadsheetArtifactError::ExportFailed {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        zip.write_all(sheet_xml(&empty).as_bytes())
            .map_err(|error| SpreadsheetArtifactError::ExportFailed {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
    } else {
        for (index, sheet) in artifact.sheets.iter().enumerate() {
            zip.start_file(format!("xl/worksheets/sheet{}.xml", index + 1), options)
                .map_err(|error| SpreadsheetArtifactError::ExportFailed {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                })?;
            zip.write_all(sheet_xml(sheet).as_bytes())
                .map_err(|error| SpreadsheetArtifactError::ExportFailed {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                })?;
        }
    }

    zip.finish()
        .map_err(|error| SpreadsheetArtifactError::ExportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    Ok(path.to_path_buf())
}

pub(crate) fn import_xlsx(
    path: &Path,
    artifact_id: Option<String>,
) -> Result<SpreadsheetArtifact, SpreadsheetArtifactError> {
    let file = File::open(path).map_err(|error| SpreadsheetArtifactError::ImportFailed {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| SpreadsheetArtifactError::ImportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

    let workbook_xml = read_zip_entry(&mut archive, "xl/workbook.xml", path)?;
    let workbook_rels = read_zip_entry(&mut archive, "xl/_rels/workbook.xml.rels", path)?;
    let workbook_name = if archive.by_name("docProps/core.xml").is_ok() {
        let title =
            extract_workbook_title(&read_zip_entry(&mut archive, "docProps/core.xml", path)?);
        (!title.trim().is_empty()).then_some(title)
    } else {
        None
    };
    let shared_strings = if archive.by_name("xl/sharedStrings.xml").is_ok() {
        Some(parse_shared_strings(&read_zip_entry(
            &mut archive,
            "xl/sharedStrings.xml",
            path,
        )?)?)
    } else {
        None
    };
    let parsed_styles = if archive.by_name("xl/styles.xml").is_ok() {
        parse_styles(&read_zip_entry(&mut archive, "xl/styles.xml", path)?)?
    } else {
        ParsedStyles::default()
    };
    let pivot_caches = parse_pivot_caches(&mut archive, path, &workbook_xml, &workbook_rels)?;
    let custom_metadata = if archive.by_name(CODEX_METADATA_PATH).is_ok() {
        Some(parse_codex_metadata(&read_zip_entry(
            &mut archive,
            CODEX_METADATA_PATH,
            path,
        )?)?)
    } else {
        None
    };

    let relationships = parse_relationships(&workbook_rels)?;
    let sheets = parse_sheet_definitions(&workbook_xml)?
        .into_iter()
        .map(|(name, relation)| {
            let target = relationships.get(&relation).ok_or_else(|| {
                SpreadsheetArtifactError::ImportFailed {
                    path: path.to_path_buf(),
                    message: format!("missing relationship `{relation}` for sheet `{name}`"),
                }
            })?;
            let normalized = if target.starts_with('/') {
                target.trim_start_matches('/').to_string()
            } else if target.starts_with("xl/") {
                target.clone()
            } else {
                format!("xl/{target}")
            };
            Ok((name, normalized))
        })
        .collect::<Result<Vec<_>, SpreadsheetArtifactError>>()?;

    let mut artifact = SpreadsheetArtifact::new(workbook_name.or_else(|| {
        path.file_stem()
            .and_then(|value| value.to_str())
            .map(str::to_string)
    }));
    if let Some(artifact_id) = artifact_id {
        artifact.artifact_id = artifact_id;
    }
    artifact.sheets.clear();
    artifact.text_styles = parsed_styles.text_styles;
    artifact.fills = parsed_styles.fills;
    artifact.borders = parsed_styles.borders;
    artifact.number_formats = parsed_styles.number_formats;
    artifact.cell_formats = parsed_styles.cell_formats;
    artifact.differential_formats = parsed_styles.differential_formats;
    let dxf_id_map = parsed_styles.dxf_id_map;

    if let Some(metadata) = custom_metadata.as_ref()
        && !metadata.differential_formats.is_empty()
    {
        artifact.differential_formats = metadata.differential_formats.clone();
    }

    for (name, target) in sheets {
        let xml = read_zip_entry(&mut archive, &target, path)?;
        let mut sheet = parse_sheet(&name, &xml, shared_strings.as_deref())?;
        sheet.charts = parse_native_charts(&mut archive, path, &target, &xml, &name)?;
        sheet.tables = parse_native_tables(&mut archive, path, &target, &xml)?;
        sheet.conditional_formats = parse_native_conditional_formats(&xml, &dxf_id_map)?;
        sheet.pivot_tables =
            parse_native_pivot_tables(&mut archive, path, &target, &xml, &pivot_caches)?;
        if let Some(metadata) = custom_metadata
            .as_ref()
            .and_then(|entry| entry.sheets.get(&name))
        {
            sheet.charts = metadata.charts.clone();
            sheet.tables = metadata.tables.clone();
            sheet.conditional_formats = metadata.conditional_formats.clone();
            sheet.pivot_tables = metadata.pivot_tables.clone();
        }
        artifact.sheets.push(sheet);
    }

    Ok(artifact)
}

fn read_zip_entry(
    archive: &mut ZipArchive<File>,
    entry: &str,
    path: &Path,
) -> Result<String, SpreadsheetArtifactError> {
    let mut file =
        archive
            .by_name(entry)
            .map_err(|error| SpreadsheetArtifactError::ImportFailed {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| SpreadsheetArtifactError::ImportFailed {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    Ok(text)
}

fn parse_sheet_definitions(
    workbook_xml: &str,
) -> Result<Vec<(String, String)>, SpreadsheetArtifactError> {
    let regex = Regex::new(r#"<sheet\b([^>]*)/?>"#).map_err(|error| {
        SpreadsheetArtifactError::Serialization {
            message: error.to_string(),
        }
    })?;
    let mut sheets = Vec::new();
    for captures in regex.captures_iter(workbook_xml) {
        let Some(attributes) = captures.get(1).map(|value| value.as_str()) else {
            continue;
        };
        let Some(name) = extract_attribute(attributes, "name") else {
            continue;
        };
        let relation = extract_attribute(attributes, "r:id")
            .or_else(|| extract_attribute(attributes, "id"))
            .unwrap_or_default();
        sheets.push((xml_unescape(&name), relation));
    }
    Ok(sheets)
}

fn parse_relationships(xml: &str) -> Result<BTreeMap<String, String>, SpreadsheetArtifactError> {
    let regex = Regex::new(r#"<Relationship\b([^>]*)/?>"#).map_err(|error| {
        SpreadsheetArtifactError::Serialization {
            message: error.to_string(),
        }
    })?;
    Ok(regex
        .captures_iter(xml)
        .filter_map(|captures| {
            let attributes = captures.get(1)?.as_str();
            let id = extract_attribute(attributes, "Id")?;
            let target = extract_attribute(attributes, "Target")?;
            Some((id, target))
        })
        .collect())
}

fn parse_shared_strings(xml: &str) -> Result<Vec<String>, SpreadsheetArtifactError> {
    let regex = Regex::new(r#"(?s)<si\b[^>]*>(.*?)</si>"#).map_err(|error| {
        SpreadsheetArtifactError::Serialization {
            message: error.to_string(),
        }
    })?;
    regex
        .captures_iter(xml)
        .filter_map(|captures| captures.get(1).map(|value| value.as_str()))
        .map(all_text_nodes)
        .collect()
}

fn parse_sheet(
    name: &str,
    xml: &str,
    shared_strings: Option<&[String]>,
) -> Result<SpreadsheetSheet, SpreadsheetArtifactError> {
    let mut sheet = SpreadsheetSheet::new(name.to_string());

    if let Some(sheet_view) = first_tag_attributes(xml, "sheetView")
        && let Some(show_grid_lines) = extract_attribute(&sheet_view, "showGridLines")
    {
        sheet.show_grid_lines = show_grid_lines != "0";
    }
    if let Some(format_pr) = first_tag_attributes(xml, "sheetFormatPr") {
        sheet.default_row_height = extract_attribute(&format_pr, "defaultRowHeight")
            .and_then(|value| value.parse::<f64>().ok());
        sheet.default_column_width = extract_attribute(&format_pr, "defaultColWidth")
            .and_then(|value| value.parse::<f64>().ok());
    }

    let col_regex = Regex::new(r#"<col\b([^>]*)/?>"#).map_err(|error| {
        SpreadsheetArtifactError::Serialization {
            message: error.to_string(),
        }
    })?;
    for captures in col_regex.captures_iter(xml) {
        let Some(attributes) = captures.get(1).map(|value| value.as_str()) else {
            continue;
        };
        let Some(min) =
            extract_attribute(attributes, "min").and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        let Some(max) =
            extract_attribute(attributes, "max").and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        let Some(width) =
            extract_attribute(attributes, "width").and_then(|value| value.parse::<f64>().ok())
        else {
            continue;
        };
        for column in min..=max {
            sheet.column_widths.insert(column, width);
        }
    }

    for (row_attributes, row_body) in child_tags(xml, "row")? {
        if let Some(row_index) =
            extract_attribute(&row_attributes, "r").and_then(|value| value.parse::<u32>().ok())
            && let Some(height) =
                extract_attribute(&row_attributes, "ht").and_then(|value| value.parse::<f64>().ok())
            && row_index > 0
            && height > 0.0
        {
            sheet.row_heights.insert(row_index, height);
        }
        for (attributes, body) in child_tags(&row_body, "c")? {
            let Some(address) = extract_attribute(&attributes, "r") else {
                continue;
            };
            let address = CellAddress::parse(&address)?;
            let style_index = extract_attribute(&attributes, "s")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            let cell_type = extract_attribute(&attributes, "t").unwrap_or_default();
            let formula = first_tag_text(&body, "f").map(|value| format!("={value}"));
            let value = parse_cell_value(&body, &cell_type, shared_strings)?;

            let cell = SpreadsheetCell {
                value,
                formula,
                style_index,
                citations: Vec::new(),
            };
            if !cell.is_empty() {
                sheet.cells.insert(address, cell);
            }
        }
    }

    let merge_regex = Regex::new(r#"<mergeCell\b([^>]*)/?>"#).map_err(|error| {
        SpreadsheetArtifactError::Serialization {
            message: error.to_string(),
        }
    })?;
    for captures in merge_regex.captures_iter(xml) {
        let Some(attributes) = captures.get(1).map(|value| value.as_str()) else {
            continue;
        };
        if let Some(reference) = extract_attribute(attributes, "ref") {
            sheet.merged_ranges.push(CellRange::parse(&reference)?);
        }
    }

    Ok(sheet)
}

fn parse_cell_value(
    body: &str,
    cell_type: &str,
    shared_strings: Option<&[String]>,
) -> Result<Option<SpreadsheetCellValue>, SpreadsheetArtifactError> {
    let inline_text = (!body.is_empty())
        .then(|| all_text_nodes(body))
        .transpose()?
        .filter(|value| !value.is_empty());
    let raw_value = first_tag_text(body, "v").map(|value| xml_unescape(&value));

    let parsed = match cell_type {
        "inlineStr" => inline_text.map(SpreadsheetCellValue::String),
        "s" => raw_value
            .and_then(|value| value.parse::<usize>().ok())
            .and_then(|index| shared_strings.and_then(|entries| entries.get(index).cloned()))
            .map(SpreadsheetCellValue::String),
        "b" => raw_value.map(|value| SpreadsheetCellValue::Bool(value == "1")),
        "str" => raw_value.map(SpreadsheetCellValue::String),
        "e" => raw_value.map(SpreadsheetCellValue::Error),
        _ => match raw_value {
            Some(value) => {
                if let Ok(integer) = value.parse::<i64>() {
                    Some(SpreadsheetCellValue::Integer(integer))
                } else if let Ok(float) = value.parse::<f64>() {
                    Some(SpreadsheetCellValue::Float(float))
                } else {
                    Some(SpreadsheetCellValue::String(value))
                }
            }
            None => None,
        },
    };
    Ok(parsed)
}

fn parse_native_tables(
    archive: &mut ZipArchive<File>,
    workbook_path: &Path,
    sheet_part: &str,
    sheet_xml: &str,
) -> Result<Vec<crate::SpreadsheetTable>, SpreadsheetArtifactError> {
    let rels_path = sheet_relationships_path(sheet_part)?;
    if archive.by_name(&rels_path).is_err() {
        return Ok(Vec::new());
    }

    let relationships = parse_relationships(&read_zip_entry(archive, &rels_path, workbook_path)?)?;
    child_tags(sheet_xml, "tablePart")?
        .into_iter()
        .filter_map(|(attributes, _)| {
            extract_attribute(&attributes, "r:id").or_else(|| extract_attribute(&attributes, "id"))
        })
        .filter_map(|relationship_id| {
            relationships.get(&relationship_id).map(|target| {
                normalize_relationship_target(sheet_part, target)
                    .map(|normalized| (relationship_id, normalized))
            })
        })
        .map(|entry| {
            let (_, table_path) = entry?;
            let xml = read_zip_entry(archive, &table_path, workbook_path)?;
            parse_native_table(&xml)
        })
        .collect()
}

fn parse_native_conditional_formats(
    sheet_xml: &str,
    dxf_id_map: &BTreeMap<u32, u32>,
) -> Result<Vec<crate::SpreadsheetConditionalFormat>, SpreadsheetArtifactError> {
    let mut formats = Vec::new();
    let mut next_id = 1;

    for (attributes, body) in child_tags(sheet_xml, "conditionalFormatting")? {
        let ranges = extract_attribute(&attributes, "sqref")
            .unwrap_or_default()
            .split_whitespace()
            .map(normalize_a1_reference)
            .collect::<Vec<_>>();
        if ranges.is_empty() {
            continue;
        }

        for (rule_attributes, rule_body) in child_tags(&body, "cfRule")? {
            let Some(rule_type) = extract_attribute(&rule_attributes, "type")
                .as_deref()
                .and_then(parse_conditional_format_type)
            else {
                continue;
            };
            let formulas = child_tags(&rule_body, "formula")?
                .into_iter()
                .map(|(_, formula)| xml_unescape(formula.trim()))
                .filter(|formula| !formula.is_empty())
                .collect::<Vec<_>>();
            let color_scale = (rule_type == crate::SpreadsheetConditionalFormatType::ColorScale)
                .then(|| parse_color_scale_rule(&rule_body))
                .transpose()?
                .flatten();
            let data_bar = (rule_type == crate::SpreadsheetConditionalFormatType::DataBar)
                .then(|| parse_data_bar_rule(&rule_body))
                .transpose()?
                .flatten();
            let icon_set = (rule_type == crate::SpreadsheetConditionalFormatType::IconSet)
                .then(|| parse_icon_set_rule(&rule_body))
                .transpose()?
                .flatten();
            let dxf_id = extract_attribute(&rule_attributes, "dxfId")
                .and_then(|value| value.parse::<u32>().ok())
                .and_then(|value| dxf_id_map.get(&value).copied());
            if dxf_id.is_none() && color_scale.is_none() && data_bar.is_none() && icon_set.is_none()
            {
                continue;
            }
            let stop_if_true = extract_attribute(&rule_attributes, "stopIfTrue")
                .and_then(|value| parse_xlsx_bool(&value))
                .unwrap_or(false);
            let priority = extract_attribute(&rule_attributes, "priority")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            let rank = extract_attribute(&rule_attributes, "rank")
                .and_then(|value| value.parse::<u32>().ok());
            let percent = extract_attribute(&rule_attributes, "percent")
                .and_then(|value| parse_xlsx_bool(&value));
            let time_period = extract_attribute(&rule_attributes, "timePeriod");
            let above_average = extract_attribute(&rule_attributes, "aboveAverage")
                .and_then(|value| parse_xlsx_bool(&value));
            let equal_average = extract_attribute(&rule_attributes, "equalAverage")
                .and_then(|value| parse_xlsx_bool(&value));
            let text = extract_attribute(&rule_attributes, "text");

            for range in &ranges {
                formats.push(crate::SpreadsheetConditionalFormat {
                    id: next_id,
                    range: range.clone(),
                    rule_type,
                    operator: extract_attribute(&rule_attributes, "operator"),
                    formulas: formulas.clone(),
                    text: text.clone(),
                    dxf_id,
                    stop_if_true,
                    priority: if priority == 0 { next_id } else { priority },
                    rank,
                    percent,
                    time_period: time_period.clone(),
                    above_average,
                    equal_average,
                    color_scale: color_scale.clone(),
                    data_bar: data_bar.clone(),
                    icon_set: icon_set.clone(),
                });
                next_id += 1;
            }
        }
    }

    Ok(formats)
}

fn parse_conditional_format_type(value: &str) -> Option<crate::SpreadsheetConditionalFormatType> {
    match value {
        "expression" => Some(crate::SpreadsheetConditionalFormatType::Expression),
        "cellIs" => Some(crate::SpreadsheetConditionalFormatType::CellIs),
        "colorScale" => Some(crate::SpreadsheetConditionalFormatType::ColorScale),
        "dataBar" => Some(crate::SpreadsheetConditionalFormatType::DataBar),
        "iconSet" => Some(crate::SpreadsheetConditionalFormatType::IconSet),
        "top10" => Some(crate::SpreadsheetConditionalFormatType::Top10),
        "uniqueValues" => Some(crate::SpreadsheetConditionalFormatType::UniqueValues),
        "duplicateValues" => Some(crate::SpreadsheetConditionalFormatType::DuplicateValues),
        "containsText" => Some(crate::SpreadsheetConditionalFormatType::ContainsText),
        "notContainsText" => Some(crate::SpreadsheetConditionalFormatType::NotContainsText),
        "beginsWith" => Some(crate::SpreadsheetConditionalFormatType::BeginsWith),
        "endsWith" => Some(crate::SpreadsheetConditionalFormatType::EndsWith),
        "containsBlanks" => Some(crate::SpreadsheetConditionalFormatType::ContainsBlanks),
        "notContainsBlanks" => Some(crate::SpreadsheetConditionalFormatType::NotContainsBlanks),
        "containsErrors" => Some(crate::SpreadsheetConditionalFormatType::ContainsErrors),
        "notContainsErrors" => Some(crate::SpreadsheetConditionalFormatType::NotContainsErrors),
        "timePeriod" => Some(crate::SpreadsheetConditionalFormatType::TimePeriod),
        "aboveAverage" => Some(crate::SpreadsheetConditionalFormatType::AboveAverage),
        _ => None,
    }
}

fn parse_color_scale_rule(
    rule_body: &str,
) -> Result<Option<crate::SpreadsheetColorScale>, SpreadsheetArtifactError> {
    let Some(color_scale) = first_tag_text(rule_body, "colorScale") else {
        return Ok(None);
    };
    let thresholds = child_tags(&color_scale, "cfvo")?
        .into_iter()
        .map(|(attributes, _)| {
            (
                extract_attribute(&attributes, "type"),
                extract_attribute(&attributes, "val"),
            )
        })
        .collect::<Vec<_>>();
    let colors = child_tags(&color_scale, "color")?
        .into_iter()
        .filter_map(|(attributes, _)| parse_color_value(&attributes))
        .collect::<Vec<_>>();
    if thresholds.len() < 2 || colors.len() < 2 {
        return Ok(None);
    }
    let mid = (thresholds.len() == 3 && colors.len() == 3).then_some(1);
    Ok(Some(crate::SpreadsheetColorScale {
        min_type: thresholds.first().and_then(|(kind, _)| kind.clone()),
        mid_type: mid.and_then(|index| thresholds.get(index).and_then(|(kind, _)| kind.clone())),
        max_type: thresholds.last().and_then(|(kind, _)| kind.clone()),
        min_value: thresholds.first().and_then(|(_, value)| value.clone()),
        mid_value: mid.and_then(|index| thresholds.get(index).and_then(|(_, value)| value.clone())),
        max_value: thresholds.last().and_then(|(_, value)| value.clone()),
        min_color: colors.first().cloned().unwrap_or_default(),
        mid_color: mid.and_then(|index| colors.get(index).cloned()),
        max_color: colors.last().cloned().unwrap_or_default(),
    }))
}

fn parse_data_bar_rule(
    rule_body: &str,
) -> Result<Option<crate::SpreadsheetDataBar>, SpreadsheetArtifactError> {
    let Some(data_bar_body) = first_tag_text(rule_body, "dataBar") else {
        return Ok(None);
    };
    let Some(data_bar_attributes) = first_tag_attributes(rule_body, "dataBar") else {
        return Ok(None);
    };
    let Some(color) = first_tag_attributes(&data_bar_body, "color")
        .and_then(|attributes| parse_color_value(&attributes))
    else {
        return Ok(None);
    };
    Ok(Some(crate::SpreadsheetDataBar {
        color,
        min_length: extract_attribute(&data_bar_attributes, "minLength")
            .and_then(|value| value.parse::<u8>().ok()),
        max_length: extract_attribute(&data_bar_attributes, "maxLength")
            .and_then(|value| value.parse::<u8>().ok()),
        show_value: extract_attribute(&data_bar_attributes, "showValue")
            .and_then(|value| parse_xlsx_bool(&value)),
    }))
}

fn parse_icon_set_rule(
    rule_body: &str,
) -> Result<Option<crate::SpreadsheetIconSet>, SpreadsheetArtifactError> {
    let Some(icon_set_attributes) = first_tag_attributes(rule_body, "iconSet") else {
        return Ok(None);
    };
    let Some(style) = extract_attribute(&icon_set_attributes, "iconSet") else {
        return Ok(None);
    };
    Ok(Some(crate::SpreadsheetIconSet {
        style,
        show_value: extract_attribute(&icon_set_attributes, "showValue")
            .and_then(|value| parse_xlsx_bool(&value)),
        reverse_order: extract_attribute(&icon_set_attributes, "reverse")
            .and_then(|value| parse_xlsx_bool(&value)),
    }))
}

fn parse_native_charts(
    archive: &mut ZipArchive<File>,
    workbook_path: &Path,
    sheet_part: &str,
    _sheet_xml: &str,
    sheet_name: &str,
) -> Result<Vec<crate::SpreadsheetChart>, SpreadsheetArtifactError> {
    let rels_path = sheet_relationships_path(sheet_part)?;
    if archive.by_name(&rels_path).is_err() {
        return Ok(Vec::new());
    }

    let relationships = parse_relationships(&read_zip_entry(archive, &rels_path, workbook_path)?)?;
    let drawing_targets = relationships
        .values()
        .filter(|target| target.contains("drawing"))
        .map(|target| normalize_relationship_target(sheet_part, target))
        .collect::<Result<Vec<_>, _>>()?;

    let mut charts = Vec::new();
    for drawing_target in drawing_targets {
        let drawing_xml = read_zip_entry(archive, &drawing_target, workbook_path)?;
        let drawing_rels_path = sheet_relationships_path(&drawing_target)?;
        let drawing_relationships = if archive.by_name(&drawing_rels_path).is_ok() {
            parse_relationships(&read_zip_entry(archive, &drawing_rels_path, workbook_path)?)?
        } else {
            BTreeMap::new()
        };

        for (relationship_id, _) in child_tags_ns(&drawing_xml, "chart")? {
            let Some(chart_rel_id) = extract_attribute(&relationship_id, "r:id")
                .or_else(|| extract_attribute(&relationship_id, "id"))
            else {
                continue;
            };
            let Some(chart_target) = drawing_relationships.get(&chart_rel_id) else {
                continue;
            };
            let chart_path = normalize_relationship_target(&drawing_target, chart_target)?;
            let chart_xml = read_zip_entry(archive, &chart_path, workbook_path)?;
            charts.push(parse_native_chart(
                &chart_xml,
                &chart_path,
                sheet_name,
                charts.len() as u32 + 1,
            )?);
        }
    }
    Ok(charts)
}

fn parse_native_pivot_tables(
    archive: &mut ZipArchive<File>,
    workbook_path: &Path,
    sheet_part: &str,
    sheet_xml: &str,
    pivot_caches: &BTreeMap<u32, crate::SpreadsheetPivotCacheDefinition>,
) -> Result<Vec<crate::SpreadsheetPivotTable>, SpreadsheetArtifactError> {
    let rels_path = sheet_relationships_path(sheet_part)?;
    if archive.by_name(&rels_path).is_err() {
        return Ok(Vec::new());
    }

    let relationships = parse_relationships(&read_zip_entry(archive, &rels_path, workbook_path)?)?;
    child_tags_ns(sheet_xml, "pivotTablePart")?
        .into_iter()
        .filter_map(|(attributes, _)| {
            extract_attribute(&attributes, "r:id").or_else(|| extract_attribute(&attributes, "id"))
        })
        .filter_map(|relationship_id| {
            relationships.get(&relationship_id).map(|target| {
                normalize_relationship_target(sheet_part, target)
                    .map(|normalized| (relationship_id, normalized))
            })
        })
        .map(|entry| {
            let (_, pivot_path) = entry?;
            let xml = read_zip_entry(archive, &pivot_path, workbook_path)?;
            parse_native_pivot_table(&xml, &pivot_path, pivot_caches)
        })
        .collect()
}

fn parse_pivot_caches(
    archive: &mut ZipArchive<File>,
    workbook_path: &Path,
    workbook_xml: &str,
    workbook_rels: &str,
) -> Result<BTreeMap<u32, crate::SpreadsheetPivotCacheDefinition>, SpreadsheetArtifactError> {
    let relationships = parse_relationships(workbook_rels)?;
    let mut caches = BTreeMap::new();
    for (attributes, _) in child_tags_ns(workbook_xml, "pivotCache")? {
        let Some(cache_id) =
            extract_attribute(&attributes, "cacheId").and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        let Some(rel_id) =
            extract_attribute(&attributes, "r:id").or_else(|| extract_attribute(&attributes, "id"))
        else {
            continue;
        };
        let Some(target) = relationships.get(&rel_id) else {
            continue;
        };
        let cache_path = normalize_relationship_target("xl/workbook.xml", target)?;
        let cache_xml = read_zip_entry(archive, &cache_path, workbook_path)?;
        let field_names = first_tag_text_ns(&cache_xml, "cacheFields")
            .map(|section| {
                child_tags_ns(&section, "cacheField").map(|entries| {
                    entries
                        .into_iter()
                        .map(|(field_attributes, _)| extract_attribute(&field_attributes, "name"))
                        .collect::<Vec<_>>()
                })
            })
            .transpose()?
            .unwrap_or_default();
        caches.insert(
            cache_id,
            crate::SpreadsheetPivotCacheDefinition {
                definition_path: cache_path,
                field_names,
            },
        );
    }
    Ok(caches)
}

fn sheet_relationships_path(sheet_part: &str) -> Result<String, SpreadsheetArtifactError> {
    let sheet_part = sheet_part.replace('\\', "/");
    let Some(parent) = Path::new(&sheet_part).parent() else {
        return Err(SpreadsheetArtifactError::Serialization {
            message: format!("sheet part `{sheet_part}` has no parent"),
        });
    };
    let Some(file_name) = Path::new(&sheet_part)
        .file_name()
        .and_then(|value| value.to_str())
    else {
        return Err(SpreadsheetArtifactError::Serialization {
            message: format!("sheet part `{sheet_part}` has no file name"),
        });
    };
    let parent = parent.to_string_lossy().replace('\\', "/");
    Ok(format!("{parent}/_rels/{file_name}.rels"))
}

fn normalize_relationship_target(
    source_part: &str,
    target: &str,
) -> Result<String, SpreadsheetArtifactError> {
    let source_part = source_part.replace('\\', "/");
    let target = target.replace('\\', "/");
    if target.starts_with('/') {
        return Ok(target.trim_start_matches('/').to_string());
    }

    let base = Path::new(&source_part).parent().ok_or_else(|| {
        SpreadsheetArtifactError::Serialization {
            message: format!("source part `{source_part}` has no parent"),
        }
    })?;
    let joined = base.join(&target);
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::Normal(value) => normalized.push(value),
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {}
        }
    }
    Ok(normalized.to_string_lossy().replace('\\', "/"))
}

fn parse_native_table(xml: &str) -> Result<crate::SpreadsheetTable, SpreadsheetArtifactError> {
    let table_attributes = first_tag_attributes(xml, "table").ok_or_else(|| {
        SpreadsheetArtifactError::Serialization {
            message: "table xml missing root table tag".to_string(),
        }
    })?;
    let id = extract_attribute(&table_attributes, "id")
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| SpreadsheetArtifactError::Serialization {
            message: "table xml missing numeric id".to_string(),
        })?;
    let name = extract_attribute(&table_attributes, "name").ok_or_else(|| {
        SpreadsheetArtifactError::Serialization {
            message: "table xml missing name".to_string(),
        }
    })?;
    let display_name =
        extract_attribute(&table_attributes, "displayName").unwrap_or_else(|| name.clone());
    let range = extract_attribute(&table_attributes, "ref").ok_or_else(|| {
        SpreadsheetArtifactError::Serialization {
            message: "table xml missing range".to_string(),
        }
    })?;
    let header_row_count = extract_attribute(&table_attributes, "headerRowCount")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(1);
    let totals_row_count = extract_attribute(&table_attributes, "totalsRowCount")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);

    let columns = first_tag_text(xml, "tableColumns")
        .map(|section| {
            child_tags(&section, "tableColumn").map(|entries| {
                entries
                    .into_iter()
                    .enumerate()
                    .map(|(index, (attributes, _))| crate::SpreadsheetTableColumn {
                        id: extract_attribute(&attributes, "id")
                            .and_then(|value| value.parse::<u32>().ok())
                            .unwrap_or(index as u32 + 1),
                        name: extract_attribute(&attributes, "name")
                            .unwrap_or_else(|| format!("Column{}", index + 1)),
                        totals_row_label: extract_attribute(&attributes, "totalsRowLabel"),
                        totals_row_function: extract_attribute(&attributes, "totalsRowFunction"),
                    })
                    .collect()
            })
        })
        .transpose()?
        .unwrap_or_default();

    let style_attributes = first_tag_attributes(xml, "tableStyleInfo");
    Ok(crate::SpreadsheetTable {
        id,
        name,
        display_name,
        range,
        header_row_count,
        totals_row_count,
        style_name: style_attributes
            .as_ref()
            .and_then(|attributes| extract_attribute(attributes, "name")),
        show_first_column: style_attributes
            .as_ref()
            .and_then(|attributes| extract_attribute(attributes, "showFirstColumn"))
            .is_some_and(|value| value == "1"),
        show_last_column: style_attributes
            .as_ref()
            .and_then(|attributes| extract_attribute(attributes, "showLastColumn"))
            .is_some_and(|value| value == "1"),
        show_row_stripes: style_attributes
            .as_ref()
            .and_then(|attributes| extract_attribute(attributes, "showRowStripes"))
            .is_some_and(|value| value == "1"),
        show_column_stripes: style_attributes
            .as_ref()
            .and_then(|attributes| extract_attribute(attributes, "showColumnStripes"))
            .is_some_and(|value| value == "1"),
        columns,
        filters: BTreeMap::new(),
    })
}

fn parse_native_chart(
    xml: &str,
    chart_path: &str,
    sheet_name: &str,
    fallback_id: u32,
) -> Result<crate::SpreadsheetChart, SpreadsheetArtifactError> {
    let style_index = first_tag_attributes_ns(xml, "style")
        .and_then(|attributes| extract_attribute(&attributes, "val"))
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(102);
    let display_blanks_as = first_tag_attributes_ns(xml, "dispBlanksAs")
        .and_then(|attributes| extract_attribute(&attributes, "val"))
        .unwrap_or_else(|| "gap".to_string());
    let title =
        first_tag_text_ns(xml, "title").and_then(|title_xml| chart_title_from_xml(&title_xml));
    let legend_visible = first_tag_attributes_ns(xml, "delete")
        .and_then(|attributes| extract_attribute(&attributes, "val"))
        .is_none_or(|value| value != "1");
    let legend_position = first_tag_attributes_ns(xml, "legendPos")
        .and_then(|attributes| extract_attribute(&attributes, "val"))
        .as_deref()
        .map(parse_chart_legend_position)
        .unwrap_or(crate::SpreadsheetChartLegendPosition::Bottom);
    let legend_overlay = first_tag_attributes_ns(xml, "overlay")
        .and_then(|attributes| extract_attribute(&attributes, "val"))
        .is_some_and(|value| value == "1");

    let (chart_type, chart_section) = chart_section(xml)?;
    let series = child_tags_ns(&chart_section, "ser")?
        .into_iter()
        .enumerate()
        .filter_map(|(index, (_, series_xml))| {
            parse_native_chart_series(&series_xml, sheet_name, index as u32 + 1).transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let source_range = infer_chart_source_range(&series, sheet_name);

    Ok(crate::SpreadsheetChart {
        id: chart_id_from_path(chart_path).unwrap_or(fallback_id),
        chart_type,
        source_sheet_name: source_range.as_ref().map(|_| sheet_name.to_string()),
        source_range,
        title,
        style_index,
        display_blanks_as,
        legend: crate::SpreadsheetChartLegend {
            visible: legend_visible,
            position: legend_position,
            overlay: legend_overlay,
        },
        category_axis: crate::SpreadsheetChartAxis {
            linked_number_format: axis_linked_number_format(xml, "catAx"),
        },
        value_axis: crate::SpreadsheetChartAxis {
            linked_number_format: axis_linked_number_format(xml, "valAx"),
        },
        series,
    })
}

fn parse_native_chart_series(
    xml: &str,
    default_sheet_name: &str,
    fallback_id: u32,
) -> Result<Option<crate::SpreadsheetChartSeries>, SpreadsheetArtifactError> {
    let category_formula = series_formula(xml, "cat");
    let value_formula = series_formula(xml, "val");
    let Some(value_formula) = value_formula else {
        return Ok(None);
    };
    let (value_sheet_name, value_range) =
        parse_formula_reference(&value_formula, Some(default_sheet_name))?;
    let (category_sheet_name, category_range) = if let Some(category_formula) = category_formula {
        parse_formula_reference(&category_formula, Some(default_sheet_name))?
    } else {
        (
            Some(default_sheet_name.to_string()),
            CellRange::from_start_end(
                CellAddress { column: 1, row: 1 },
                CellAddress { column: 1, row: 1 },
            )
            .to_a1(),
        )
    };

    Ok(Some(crate::SpreadsheetChartSeries {
        id: first_tag_attributes_ns(xml, "idx")
            .and_then(|attributes| extract_attribute(&attributes, "val"))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(fallback_id),
        name: chart_series_name(xml),
        category_sheet_name,
        category_range,
        value_sheet_name,
        value_range,
    }))
}

fn parse_native_pivot_table(
    xml: &str,
    pivot_path: &str,
    pivot_caches: &BTreeMap<u32, crate::SpreadsheetPivotCacheDefinition>,
) -> Result<crate::SpreadsheetPivotTable, SpreadsheetArtifactError> {
    let attributes = first_tag_attributes_ns(xml, "pivotTableDefinition").ok_or_else(|| {
        SpreadsheetArtifactError::Serialization {
            message: "pivot table definition missing root tag".to_string(),
        }
    })?;
    let name = extract_attribute(&attributes, "name").ok_or_else(|| {
        SpreadsheetArtifactError::Serialization {
            message: "pivot table definition missing name".to_string(),
        }
    })?;
    let cache_id = extract_attribute(&attributes, "cacheId")
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| SpreadsheetArtifactError::Serialization {
            message: "pivot table definition missing cacheId".to_string(),
        })?;
    let cache_names = pivot_caches
        .get(&cache_id)
        .map(|cache| cache.field_names.clone())
        .unwrap_or_default();

    let pivot_fields = first_tag_text_ns(xml, "pivotFields")
        .map(|section| {
            child_tags_ns(&section, "pivotField").map(|entries| {
                entries
                    .into_iter()
                    .enumerate()
                    .map(
                        |(index, (field_attributes, field_xml))| crate::SpreadsheetPivotField {
                            index: index as u32,
                            name: cache_names.get(index).cloned().flatten(),
                            axis: extract_attribute(&field_attributes, "axis"),
                            items: first_tag_text_ns(&field_xml, "items")
                                .map(|items_xml| {
                                    child_tags_ns(&items_xml, "item").map(|items| {
                                        items
                                            .into_iter()
                                            .map(|(item_attributes, _)| {
                                                crate::SpreadsheetPivotFieldItem {
                                                    item_type: extract_attribute(
                                                        &item_attributes,
                                                        "t",
                                                    ),
                                                    index: extract_attribute(&item_attributes, "x")
                                                        .and_then(|value| {
                                                            value.parse::<u32>().ok()
                                                        }),
                                                    hidden: extract_attribute(
                                                        &item_attributes,
                                                        "h",
                                                    )
                                                    .is_some_and(|value| value == "1"),
                                                }
                                            })
                                            .collect()
                                    })
                                })
                                .transpose()
                                .unwrap_or_default()
                                .unwrap_or_default(),
                        },
                    )
                    .collect()
            })
        })
        .transpose()?
        .unwrap_or_default();

    Ok(crate::SpreadsheetPivotTable {
        name,
        cache_id,
        address: first_tag_attributes_ns(xml, "location")
            .and_then(|location_attributes| extract_attribute(&location_attributes, "ref")),
        row_fields: field_references_from_section(
            first_tag_text_ns(xml, "rowFields").as_deref(),
            "field",
            "x",
            &cache_names,
        )?,
        column_fields: field_references_from_section(
            first_tag_text_ns(xml, "colFields").as_deref(),
            "field",
            "x",
            &cache_names,
        )?,
        page_fields: page_fields_from_section(
            first_tag_text_ns(xml, "pageFields").as_deref(),
            &cache_names,
        )?,
        data_fields: data_fields_from_section(
            first_tag_text_ns(xml, "dataFields").as_deref(),
            &cache_names,
        )?,
        filters: pivot_filters_from_section(
            first_tag_text_ns(xml, "filters").as_deref(),
            &cache_names,
        )?,
        pivot_fields,
        style_name: first_tag_attributes_ns(xml, "pivotTableStyleInfo")
            .and_then(|style_attributes| extract_attribute(&style_attributes, "name")),
        part_path: Some(pivot_path.to_string()),
    })
}

fn chart_section(
    xml: &str,
) -> Result<(crate::SpreadsheetChartType, String), SpreadsheetArtifactError> {
    for (tag, chart_type) in [
        ("areaChart", crate::SpreadsheetChartType::Area),
        ("barChart", crate::SpreadsheetChartType::Bar),
        ("doughnutChart", crate::SpreadsheetChartType::Doughnut),
        ("lineChart", crate::SpreadsheetChartType::Line),
        ("pieChart", crate::SpreadsheetChartType::Pie),
    ] {
        if let Some(section) = first_tag_text_ns(xml, tag) {
            return Ok((chart_type, section));
        }
    }
    Err(SpreadsheetArtifactError::Serialization {
        message: "chart xml did not contain a supported chart type".to_string(),
    })
}

fn chart_series_name(xml: &str) -> Option<String> {
    let tx = first_tag_text_ns(xml, "tx")?;
    first_tag_text_ns(&tx, "v")
        .map(|value| xml_unescape(&value))
        .or_else(|| {
            first_tag_text_ns(&tx, "f").map(|formula| {
                parse_formula_reference(&formula, None)
                    .map(|(_, range)| range)
                    .unwrap_or_else(|_| formula)
            })
        })
        .or_else(|| {
            all_text_nodes_any_ns(&tx)
                .ok()
                .filter(|value| !value.is_empty())
        })
}

fn series_formula(xml: &str, axis_tag: &str) -> Option<String> {
    let axis_xml = first_tag_text_ns(xml, axis_tag)?;
    ["strRef", "numRef", "multiLvlStrRef"]
        .into_iter()
        .find_map(|ref_tag| first_tag_text_ns(&axis_xml, ref_tag))
        .and_then(|reference_xml| first_tag_text_ns(&reference_xml, "f"))
}

fn chart_title_from_xml(xml: &str) -> Option<String> {
    first_tag_text_ns(xml, "v")
        .map(|value| xml_unescape(&value))
        .or_else(|| {
            all_text_nodes_any_ns(xml)
                .ok()
                .filter(|value| !value.is_empty())
        })
}

fn parse_chart_legend_position(value: &str) -> crate::SpreadsheetChartLegendPosition {
    match value {
        "t" => crate::SpreadsheetChartLegendPosition::Top,
        "l" => crate::SpreadsheetChartLegendPosition::Left,
        "r" => crate::SpreadsheetChartLegendPosition::Right,
        _ => crate::SpreadsheetChartLegendPosition::Bottom,
    }
}

fn axis_linked_number_format(xml: &str, axis_tag: &str) -> bool {
    first_tag_text_ns(xml, axis_tag)
        .and_then(|axis_xml| first_tag_attributes_ns(&axis_xml, "numFmt"))
        .and_then(|attributes| extract_attribute(&attributes, "sourceLinked"))
        .is_none_or(|value| value != "0")
}

fn chart_id_from_path(path: &str) -> Option<u32> {
    Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .and_then(|value| value.trim_start_matches("chart").parse::<u32>().ok())
}

fn infer_chart_source_range(
    series: &[crate::SpreadsheetChartSeries],
    sheet_name: &str,
) -> Option<String> {
    let first = series.first()?;
    let category_sheet_name = first.category_sheet_name.as_deref()?;
    if category_sheet_name != sheet_name {
        return None;
    }
    let category_range = CellRange::parse(&first.category_range).ok()?;
    let mut columns = vec![category_range.start.column];
    let mut row_start = category_range.start.row;
    let mut row_end = category_range.end.row;

    for entry in series {
        if entry.category_sheet_name.as_deref() != Some(sheet_name)
            || entry.value_sheet_name.as_deref() != Some(sheet_name)
        {
            return None;
        }
        let category = CellRange::parse(&entry.category_range).ok()?;
        let value = CellRange::parse(&entry.value_range).ok()?;
        if category != category_range
            || !value.is_single_column()
            || value.start.row != row_start
            || value.end.row != row_end
        {
            return None;
        }
        columns.push(value.start.column);
        row_start = row_start.min(value.start.row);
        row_end = row_end.max(value.end.row);
    }

    columns.sort_unstable();
    columns.dedup();
    if columns.len() != series.len() + 1 {
        return None;
    }
    let start = *columns.first()?;
    let end = *columns.last()?;
    if columns != (start..=end).collect::<Vec<_>>() {
        return None;
    }
    Some(
        CellRange::from_start_end(
            CellAddress {
                column: start,
                row: row_start,
            },
            CellAddress {
                column: end,
                row: row_end,
            },
        )
        .to_a1(),
    )
}

fn parse_formula_reference(
    formula: &str,
    default_sheet_name: Option<&str>,
) -> Result<(Option<String>, String), SpreadsheetArtifactError> {
    let trimmed = formula.trim().trim_start_matches('=');
    if let Some((sheet_name, range)) = trimmed.split_once('!') {
        return Ok((
            Some(sheet_name.trim_matches('\'').replace("''", "'")),
            normalize_a1_reference(range),
        ));
    }
    Ok((
        default_sheet_name.map(str::to_string),
        normalize_a1_reference(trimmed),
    ))
}

fn normalize_a1_reference(reference: &str) -> String {
    reference.replace('$', "")
}

fn field_references_from_section(
    section: Option<&str>,
    child_tag: &str,
    field_attr: &str,
    cache_names: &[Option<String>],
) -> Result<Vec<crate::SpreadsheetPivotFieldReference>, SpreadsheetArtifactError> {
    let Some(section) = section else {
        return Ok(Vec::new());
    };
    Ok(child_tags_ns(section, child_tag)?
        .into_iter()
        .filter_map(|(attributes, _)| {
            extract_attribute(&attributes, field_attr)
                .and_then(|value| value.parse::<u32>().ok())
                .map(|field_index| crate::SpreadsheetPivotFieldReference {
                    field_index,
                    field_name: cache_names.get(field_index as usize).cloned().flatten(),
                })
        })
        .collect::<Vec<_>>())
}

fn page_fields_from_section(
    section: Option<&str>,
    cache_names: &[Option<String>],
) -> Result<Vec<crate::SpreadsheetPivotPageField>, SpreadsheetArtifactError> {
    let Some(section) = section else {
        return Ok(Vec::new());
    };
    Ok(child_tags_ns(section, "pageField")?
        .into_iter()
        .filter_map(|(attributes, _)| {
            extract_attribute(&attributes, "fld")
                .and_then(|value| value.parse::<u32>().ok())
                .map(|field_index| crate::SpreadsheetPivotPageField {
                    field_index,
                    field_name: cache_names.get(field_index as usize).cloned().flatten(),
                    selected_item: extract_attribute(&attributes, "item")
                        .and_then(|value| value.parse::<u32>().ok()),
                })
        })
        .collect::<Vec<_>>())
}

fn data_fields_from_section(
    section: Option<&str>,
    cache_names: &[Option<String>],
) -> Result<Vec<crate::SpreadsheetPivotDataField>, SpreadsheetArtifactError> {
    let Some(section) = section else {
        return Ok(Vec::new());
    };
    Ok(child_tags_ns(section, "dataField")?
        .into_iter()
        .filter_map(|(attributes, _)| {
            extract_attribute(&attributes, "fld")
                .and_then(|value| value.parse::<u32>().ok())
                .map(|field_index| crate::SpreadsheetPivotDataField {
                    field_index,
                    field_name: cache_names.get(field_index as usize).cloned().flatten(),
                    name: extract_attribute(&attributes, "name"),
                    subtotal: extract_attribute(&attributes, "subtotal"),
                })
        })
        .collect::<Vec<_>>())
}

fn pivot_filters_from_section(
    section: Option<&str>,
    cache_names: &[Option<String>],
) -> Result<Vec<crate::SpreadsheetPivotFilter>, SpreadsheetArtifactError> {
    let Some(section) = section else {
        return Ok(Vec::new());
    };
    Ok(child_tags_ns(section, "filter")?
        .into_iter()
        .map(|(attributes, _)| {
            let field_index =
                extract_attribute(&attributes, "fld").and_then(|value| value.parse::<u32>().ok());
            crate::SpreadsheetPivotFilter {
                field_index,
                field_name: field_index
                    .and_then(|index| cache_names.get(index as usize).cloned().flatten()),
                filter_type: extract_attribute(&attributes, "type"),
            }
        })
        .collect::<Vec<_>>())
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct CodexSpreadsheetMetadata {
    #[serde(default)]
    sheets: BTreeMap<String, CodexSheetMetadata>,
    #[serde(default)]
    differential_formats: BTreeMap<u32, crate::SpreadsheetDifferentialFormat>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct CodexSheetMetadata {
    #[serde(default)]
    charts: Vec<crate::SpreadsheetChart>,
    #[serde(default)]
    tables: Vec<crate::SpreadsheetTable>,
    #[serde(default)]
    conditional_formats: Vec<crate::SpreadsheetConditionalFormat>,
    #[serde(default)]
    pivot_tables: Vec<crate::SpreadsheetPivotTable>,
}

#[derive(Default)]
struct ParsedStyles {
    text_styles: BTreeMap<u32, SpreadsheetTextStyle>,
    fills: BTreeMap<u32, SpreadsheetFill>,
    borders: BTreeMap<u32, SpreadsheetBorder>,
    number_formats: BTreeMap<u32, SpreadsheetNumberFormat>,
    cell_formats: BTreeMap<u32, SpreadsheetCellFormat>,
    differential_formats: BTreeMap<u32, crate::SpreadsheetDifferentialFormat>,
    dxf_id_map: BTreeMap<u32, u32>,
}

#[derive(Default)]
struct ParsedCellFormat {
    font_index: Option<u32>,
    fill_index: Option<u32>,
    border_index: Option<u32>,
    number_format_index: Option<u32>,
    alignment: Option<SpreadsheetAlignment>,
    wrap_text: Option<bool>,
}

type ParsedDifferentialFormats = (
    BTreeMap<u32, crate::SpreadsheetDifferentialFormat>,
    BTreeMap<u32, u32>,
);

fn parse_styles(xml: &str) -> Result<ParsedStyles, SpreadsheetArtifactError> {
    let custom_number_formats = parse_custom_number_formats(xml)?;
    let fonts = parse_fonts(xml)?;
    let fills = parse_fills(xml)?;
    let borders = parse_borders(xml)?;
    let xfs = parse_cell_formats(xml)?;

    let mut parsed = ParsedStyles::default();
    let mut font_id_map = BTreeMap::new();
    let mut fill_id_map = BTreeMap::new();
    let mut border_id_map = BTreeMap::new();
    let mut number_format_id_map = BTreeMap::new();

    for (index, font) in fonts.into_iter().enumerate() {
        if index == 0 || font == SpreadsheetTextStyle::default() {
            continue;
        }
        let internal_id = next_style_component_id(&parsed.text_styles);
        parsed.text_styles.insert(internal_id, font);
        font_id_map.insert(index as u32, internal_id);
    }

    for (index, fill) in fills.into_iter().enumerate() {
        if index < 2 || fill == SpreadsheetFill::default() {
            continue;
        }
        let internal_id = next_style_component_id(&parsed.fills);
        parsed.fills.insert(internal_id, fill);
        fill_id_map.insert(index as u32, internal_id);
    }

    for (index, border) in borders.into_iter().enumerate() {
        if index == 0 || border == SpreadsheetBorder::default() {
            continue;
        }
        let internal_id = next_style_component_id(&parsed.borders);
        parsed.borders.insert(internal_id, border);
        border_id_map.insert(index as u32, internal_id);
    }

    for (style_index, xf) in xfs.into_iter().enumerate() {
        let number_format_id = xf.number_format_index.and_then(|num_fmt_id| {
            resolve_number_format_id(
                num_fmt_id,
                &custom_number_formats,
                &mut parsed.number_formats,
                &mut number_format_id_map,
            )
        });

        let format = SpreadsheetCellFormat {
            text_style_id: xf
                .font_index
                .and_then(|value| font_id_map.get(&value).copied()),
            fill_id: xf
                .fill_index
                .and_then(|value| fill_id_map.get(&value).copied()),
            border_id: xf
                .border_index
                .and_then(|value| border_id_map.get(&value).copied()),
            alignment: xf.alignment,
            number_format_id,
            wrap_text: xf.wrap_text,
            base_cell_style_format_id: None,
        };

        if style_index != 0 || format != SpreadsheetCellFormat::default() {
            parsed.cell_formats.insert(style_index as u32, format);
        }
    }

    let (differential_formats, dxf_id_map) = parse_differential_formats(
        xml,
        &mut parsed.text_styles,
        &mut parsed.fills,
        &mut parsed.borders,
        &mut parsed.number_formats,
        &custom_number_formats,
        &mut number_format_id_map,
    )?;
    parsed.differential_formats = differential_formats;
    parsed.dxf_id_map = dxf_id_map;

    Ok(parsed)
}

fn parse_custom_number_formats(
    xml: &str,
) -> Result<BTreeMap<u32, String>, SpreadsheetArtifactError> {
    let Some(section) = first_tag_text(xml, "numFmts") else {
        return Ok(BTreeMap::new());
    };

    let mut number_formats = BTreeMap::new();
    for (attributes, _) in child_tags(&section, "numFmt")? {
        let Some(num_fmt_id) =
            extract_attribute(&attributes, "numFmtId").and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        let Some(format_code) = extract_attribute(&attributes, "formatCode") else {
            continue;
        };
        number_formats.insert(num_fmt_id, format_code);
    }
    Ok(number_formats)
}

fn parse_fonts(xml: &str) -> Result<Vec<SpreadsheetTextStyle>, SpreadsheetArtifactError> {
    let Some(section) = first_tag_text(xml, "fonts") else {
        return Ok(Vec::new());
    };

    child_tags(&section, "font")?
        .into_iter()
        .map(|(_, body)| Ok(parse_font_body(&body)))
        .collect()
}

fn parse_fills(xml: &str) -> Result<Vec<SpreadsheetFill>, SpreadsheetArtifactError> {
    let Some(section) = first_tag_text(xml, "fills") else {
        return Ok(Vec::new());
    };

    child_tags(&section, "fill")?
        .into_iter()
        .map(|(_, body)| parse_fill_body(&body))
        .collect()
}

fn parse_borders(xml: &str) -> Result<Vec<SpreadsheetBorder>, SpreadsheetArtifactError> {
    let Some(section) = first_tag_text(xml, "borders") else {
        return Ok(Vec::new());
    };

    child_tags(&section, "border")?
        .into_iter()
        .map(|(_, body)| Ok(parse_border_body(&body)))
        .collect()
}

fn parse_font_body(body: &str) -> SpreadsheetTextStyle {
    let name = first_tag_attributes(body, "name")
        .and_then(|attributes| extract_attribute(&attributes, "val"));
    SpreadsheetTextStyle {
        bold: first_tag_attributes(body, "b").map(|_| true),
        italic: first_tag_attributes(body, "i").map(|_| true),
        underline: first_tag_attributes(body, "u").map(|_| true),
        font_size: first_tag_attributes(body, "sz")
            .and_then(|attributes| extract_attribute(&attributes, "val"))
            .and_then(|value| value.parse::<f64>().ok()),
        font_color: first_tag_attributes(body, "color")
            .and_then(|attributes| parse_color_value(&attributes)),
        font_family: name.clone(),
        typeface: name,
        font_scheme: first_tag_attributes(body, "scheme")
            .and_then(|attributes| extract_attribute(&attributes, "val")),
        ..Default::default()
    }
}

fn parse_fill_body(body: &str) -> Result<SpreadsheetFill, SpreadsheetArtifactError> {
    let mut fill = SpreadsheetFill::default();
    if let Some(pattern_attributes) = first_tag_attributes(body, "patternFill") {
        fill.pattern_type = extract_attribute(&pattern_attributes, "patternType");
        fill.pattern_foreground_color = first_tag_attributes(body, "fgColor")
            .and_then(|attributes| parse_color_value(&attributes));
        fill.pattern_background_color = first_tag_attributes(body, "bgColor")
            .and_then(|attributes| parse_color_value(&attributes));
        fill.solid_fill_color = fill.pattern_foreground_color.clone();
    }
    if let Some(gradient_attributes) = first_tag_attributes(body, "gradientFill") {
        fill.gradient_fill_type = Some(
            extract_attribute(&gradient_attributes, "type").unwrap_or_else(|| "linear".to_string()),
        );
        fill.gradient_kind = fill.gradient_fill_type.clone();
        fill.angle = extract_attribute(&gradient_attributes, "degree")
            .and_then(|value| value.parse::<f64>().ok());
        let rectangle = SpreadsheetFillRectangle {
            left: extract_attribute(&gradient_attributes, "left")
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0),
            right: extract_attribute(&gradient_attributes, "right")
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0),
            top: extract_attribute(&gradient_attributes, "top")
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0),
            bottom: extract_attribute(&gradient_attributes, "bottom")
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0),
        };
        if rectangle.left != 0.0
            || rectangle.right != 0.0
            || rectangle.top != 0.0
            || rectangle.bottom != 0.0
        {
            fill.fill_rectangle = Some(rectangle);
        }
        fill.gradient_stops = child_tags(body, "stop")?
            .into_iter()
            .filter_map(|(attributes, stop_body)| {
                let position = extract_attribute(&attributes, "position")
                    .and_then(|value| value.parse::<f64>().ok())?;
                let color = first_tag_attributes(&stop_body, "color")
                    .and_then(|stop_attributes| parse_color_value(&stop_attributes))?;
                Some(SpreadsheetGradientStop { position, color })
            })
            .collect();
    }
    Ok(fill)
}

fn parse_border_body(body: &str) -> SpreadsheetBorder {
    SpreadsheetBorder {
        top: parse_border_line(body, "top"),
        right: parse_border_line(body, "right"),
        bottom: parse_border_line(body, "bottom"),
        left: parse_border_line(body, "left"),
    }
}

fn parse_border_line(body: &str, edge: &str) -> Option<SpreadsheetBorderLine> {
    let mut matches = child_tags(body, edge).ok()?;
    let (attributes, edge_body) = matches.pop()?;
    let style = extract_attribute(&attributes, "style");
    let color = first_tag_attributes(&edge_body, "color")
        .and_then(|color_attributes| parse_color_value(&color_attributes));
    if style.is_none() && color.is_none() {
        None
    } else {
        Some(SpreadsheetBorderLine { style, color })
    }
}

fn parse_alignment_attributes(attributes: &str) -> (Option<SpreadsheetAlignment>, Option<bool>) {
    let alignment = SpreadsheetAlignment {
        horizontal: extract_attribute(attributes, "horizontal"),
        vertical: extract_attribute(attributes, "vertical"),
    };
    let wrap_text =
        extract_attribute(attributes, "wrapText").and_then(|value| parse_xlsx_bool(&value));
    (
        (alignment != SpreadsheetAlignment::default()).then_some(alignment),
        wrap_text,
    )
}

fn parse_xlsx_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "true" | "TRUE" => Some(true),
        "0" | "false" | "FALSE" => Some(false),
        _ => None,
    }
}

fn parse_cell_formats(xml: &str) -> Result<Vec<ParsedCellFormat>, SpreadsheetArtifactError> {
    let Some(section) = first_tag_text(xml, "cellXfs") else {
        return Ok(Vec::new());
    };

    child_tags(&section, "xf")?
        .into_iter()
        .map(|(attributes, body)| {
            let (alignment, wrap_text) = first_tag_attributes(&body, "alignment")
                .map(|attributes| parse_alignment_attributes(&attributes))
                .unwrap_or_default();

            Ok(ParsedCellFormat {
                font_index: extract_attribute(&attributes, "fontId")
                    .and_then(|value| value.parse::<u32>().ok()),
                fill_index: extract_attribute(&attributes, "fillId")
                    .and_then(|value| value.parse::<u32>().ok()),
                border_index: extract_attribute(&attributes, "borderId")
                    .and_then(|value| value.parse::<u32>().ok()),
                number_format_index: extract_attribute(&attributes, "numFmtId")
                    .and_then(|value| value.parse::<u32>().ok()),
                alignment,
                wrap_text,
            })
        })
        .collect()
}

fn parse_differential_formats(
    xml: &str,
    text_styles: &mut BTreeMap<u32, SpreadsheetTextStyle>,
    fills: &mut BTreeMap<u32, SpreadsheetFill>,
    borders: &mut BTreeMap<u32, SpreadsheetBorder>,
    number_formats: &mut BTreeMap<u32, SpreadsheetNumberFormat>,
    custom_number_formats: &BTreeMap<u32, String>,
    number_format_id_map: &mut BTreeMap<u32, u32>,
) -> Result<ParsedDifferentialFormats, SpreadsheetArtifactError> {
    let Some(section) = first_tag_text(xml, "dxfs") else {
        return Ok((BTreeMap::new(), BTreeMap::new()));
    };

    let mut differential_formats = BTreeMap::new();
    let mut dxf_id_map = BTreeMap::new();
    for (xlsx_dxf_index, (_, body)) in child_tags(&section, "dxf")?.into_iter().enumerate() {
        let text_style_id = first_tag_text(&body, "font")
            .and_then(|font_body| insert_style_component(text_styles, parse_font_body(&font_body)));
        let fill_id = first_tag_text(&body, "fill")
            .map(|fill_body| parse_fill_body(&fill_body))
            .transpose()?
            .and_then(|fill| insert_style_component(fills, fill));
        let border_id = first_tag_text(&body, "border")
            .map(|border_body| parse_border_body(&border_body))
            .and_then(|border| insert_style_component(borders, border));
        let number_format_id = first_tag_attributes(&body, "numFmt")
            .and_then(|attributes| extract_attribute(&attributes, "numFmtId"))
            .and_then(|value| value.parse::<u32>().ok())
            .and_then(|num_fmt_id| {
                resolve_number_format_id(
                    num_fmt_id,
                    custom_number_formats,
                    number_formats,
                    number_format_id_map,
                )
            });
        let (alignment, wrap_text) = first_tag_attributes(&body, "alignment")
            .map(|attributes| parse_alignment_attributes(&attributes))
            .unwrap_or_default();
        let format = crate::SpreadsheetDifferentialFormat {
            text_style_id,
            fill_id,
            border_id,
            alignment,
            number_format_id,
            wrap_text,
        };
        if format == crate::SpreadsheetDifferentialFormat::default() {
            continue;
        }
        let internal_id = next_style_component_id(&differential_formats);
        differential_formats.insert(internal_id, format);
        dxf_id_map.insert(xlsx_dxf_index as u32, internal_id);
    }
    Ok((differential_formats, dxf_id_map))
}

fn insert_style_component<T>(map: &mut BTreeMap<u32, T>, value: T) -> Option<u32>
where
    T: Default + PartialEq,
{
    if value == T::default() {
        return None;
    }
    let internal_id = next_style_component_id(map);
    map.insert(internal_id, value);
    Some(internal_id)
}

fn resolve_number_format_id(
    num_fmt_id: u32,
    custom_number_formats: &BTreeMap<u32, String>,
    number_formats: &mut BTreeMap<u32, SpreadsheetNumberFormat>,
    number_format_id_map: &mut BTreeMap<u32, u32>,
) -> Option<u32> {
    if num_fmt_id == 0 {
        return None;
    }
    if let Some(existing) = number_format_id_map.get(&num_fmt_id).copied() {
        return Some(existing);
    }
    let internal_id = next_style_component_id(number_formats);
    number_formats.insert(
        internal_id,
        SpreadsheetNumberFormat {
            format_id: Some(num_fmt_id),
            format_code: custom_number_formats
                .get(&num_fmt_id)
                .cloned()
                .or_else(|| builtin_number_format_code_for_xlsx(num_fmt_id)),
        },
    );
    number_format_id_map.insert(num_fmt_id, internal_id);
    Some(internal_id)
}

fn next_style_component_id<T>(map: &BTreeMap<u32, T>) -> u32 {
    map.last_key_value().map(|(id, _)| id + 1).unwrap_or(1)
}

fn content_types_xml(sheet_count: usize, include_custom_metadata: bool) -> String {
    let mut overrides = String::new();
    for index in 1..=sheet_count {
        overrides.push_str(&format!(
            r#"<Override PartName="/xl/worksheets/sheet{index}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#
        ));
    }
    if include_custom_metadata {
        overrides.push_str(
            r#"<Override PartName="/customXml/item1.xml" ContentType="application/xml"/>"#,
        );
    }
    format!(
        "{}{}{}{}{}{}{}{}{}{}",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
        r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
        r#"<Default Extension="xml" ContentType="application/xml"/>"#,
        r#"<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#,
        r#"<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#,
        r#"<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>"#,
        r#"<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#,
        overrides,
        r#"</Types>"#
    )
}

fn artifact_has_custom_metadata(artifact: &SpreadsheetArtifact) -> bool {
    artifact.sheets.iter().any(|sheet| {
        !sheet.charts.is_empty()
            || !sheet.tables.is_empty()
            || !sheet.conditional_formats.is_empty()
            || !sheet.pivot_tables.is_empty()
    })
}

fn codex_metadata_xml(artifact: &SpreadsheetArtifact) -> Result<String, SpreadsheetArtifactError> {
    let metadata = CodexSpreadsheetMetadata {
        differential_formats: artifact.differential_formats.clone(),
        sheets: artifact
            .sheets
            .iter()
            .filter_map(|sheet| {
                let sheet_metadata = CodexSheetMetadata {
                    charts: sheet.charts.clone(),
                    tables: sheet.tables.clone(),
                    conditional_formats: sheet.conditional_formats.clone(),
                    pivot_tables: sheet.pivot_tables.clone(),
                };
                (!sheet_metadata.charts.is_empty()
                    || !sheet_metadata.tables.is_empty()
                    || !sheet_metadata.conditional_formats.is_empty()
                    || !sheet_metadata.pivot_tables.is_empty())
                .then_some((sheet.name.clone(), sheet_metadata))
            })
            .collect(),
    };
    let json = serde_json::to_string(&metadata).map_err(|error| {
        SpreadsheetArtifactError::Serialization {
            message: error.to_string(),
        }
    })?;
    Ok(format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<codexSpreadsheetMetadata xmlns="https://openai.com/codex/spreadsheet">"#,
            r#"<json>{}</json>"#,
            r#"</codexSpreadsheetMetadata>"#
        ),
        xml_escape(&json)
    ))
}

fn parse_codex_metadata(xml: &str) -> Result<CodexSpreadsheetMetadata, SpreadsheetArtifactError> {
    let Some(json) = first_tag_text(xml, "json") else {
        return Ok(CodexSpreadsheetMetadata::default());
    };
    serde_json::from_str(&xml_unescape(&json)).map_err(|error| {
        SpreadsheetArtifactError::Serialization {
            message: error.to_string(),
        }
    })
}

fn root_relationships_xml() -> &'static str {
    concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
        r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>"#,
        r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>"#,
        r#"<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>"#,
        r#"</Relationships>"#
    )
}

fn app_xml(artifact: &SpreadsheetArtifact) -> String {
    let title = artifact
        .name
        .clone()
        .unwrap_or_else(|| "Spreadsheet".to_string());
    format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">"#,
            r#"<Application>Codex</Application>"#,
            r#"<DocSecurity>0</DocSecurity>"#,
            r#"<ScaleCrop>false</ScaleCrop>"#,
            r#"<HeadingPairs><vt:vector size="2" baseType="variant"><vt:variant><vt:lpstr>Worksheets</vt:lpstr></vt:variant><vt:variant><vt:i4>{}</vt:i4></vt:variant></vt:vector></HeadingPairs>"#,
            r#"<TitlesOfParts><vt:vector size="{}" baseType="lpstr">{}</vt:vector></TitlesOfParts>"#,
            r#"<Company>OpenAI</Company>"#,
            r#"<Manager>{}</Manager>"#,
            r#"</Properties>"#
        ),
        artifact.sheets.len(),
        artifact.sheets.len(),
        artifact
            .sheets
            .iter()
            .map(|sheet| format!(r#"<vt:lpstr>{}</vt:lpstr>"#, xml_escape(&sheet.name)))
            .collect::<Vec<_>>()
            .join(""),
        xml_escape(&title),
    )
}

fn core_xml(artifact: &SpreadsheetArtifact) -> String {
    let title = artifact
        .name
        .clone()
        .unwrap_or_else(|| artifact.artifact_id.clone());
    format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">"#,
            r#"<dc:title>{}</dc:title>"#,
            r#"<dc:creator>Codex</dc:creator>"#,
            r#"<cp:lastModifiedBy>Codex</cp:lastModifiedBy>"#,
            r#"</cp:coreProperties>"#
        ),
        xml_escape(&title),
    )
}

fn workbook_xml(artifact: &SpreadsheetArtifact) -> String {
    let sheets = if artifact.sheets.is_empty() {
        r#"<sheet name="Sheet1" sheetId="1" r:id="rId1"/>"#.to_string()
    } else {
        artifact
            .sheets
            .iter()
            .enumerate()
            .map(|(index, sheet)| {
                format!(
                    r#"<sheet name="{}" sheetId="{}" r:id="rId{}"/>"#,
                    xml_escape(&sheet.name),
                    index + 1,
                    index + 1
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };
    format!(
        "{}{}{}<sheets>{}</sheets>{}",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
        r#"<bookViews><workbookView/></bookViews>"#,
        sheets,
        r#"</workbook>"#
    )
}

fn workbook_relationships_xml(artifact: &SpreadsheetArtifact) -> String {
    let sheet_relationships = if artifact.sheets.is_empty() {
        r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>"#.to_string()
    } else {
        artifact
            .sheets
            .iter()
            .enumerate()
            .map(|(index, _)| {
                format!(
                    r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{}.xml"/>"#,
                    index + 1,
                    index + 1
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };
    let style_relation_id = artifact.sheets.len().max(1) + 1;
    format!(
        "{}{}{}<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>{}",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
        sheet_relationships,
        style_relation_id,
        r#"</Relationships>"#
    )
}

fn styles_xml(artifact: &SpreadsheetArtifact) -> String {
    let max_style_index = artifact
        .sheets
        .iter()
        .flat_map(|sheet| sheet.cells.values().map(|cell| cell.style_index))
        .chain(artifact.cell_formats.keys().copied())
        .max()
        .unwrap_or(0);
    let font_ids = artifact.text_styles.keys().copied().collect::<Vec<_>>();
    let fill_ids = artifact.fills.keys().copied().collect::<Vec<_>>();
    let border_ids = artifact.borders.keys().copied().collect::<Vec<_>>();
    let number_format_ids = artifact.number_formats.keys().copied().collect::<Vec<_>>();
    let differential_format_ids = artifact
        .differential_formats
        .keys()
        .copied()
        .collect::<Vec<_>>();

    let font_indices = font_ids
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index as u32 + 1))
        .collect::<BTreeMap<_, _>>();
    let fill_indices = fill_ids
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index as u32 + 2))
        .collect::<BTreeMap<_, _>>();
    let border_indices = border_ids
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index as u32 + 1))
        .collect::<BTreeMap<_, _>>();
    let number_format_indices = assign_number_format_ids(artifact, &number_format_ids);

    let fonts = std::iter::once("<font/>".to_string())
        .chain(
            font_ids
                .iter()
                .filter_map(|id| artifact.text_styles.get(id).map(spreadsheet_font_xml)),
        )
        .collect::<Vec<_>>()
        .join("");
    let fills = [
        r#"<fill><patternFill patternType="none"/></fill>"#.to_string(),
        r#"<fill><patternFill patternType="gray125"/></fill>"#.to_string(),
    ]
    .into_iter()
    .chain(
        fill_ids
            .iter()
            .filter_map(|id| artifact.fills.get(id).map(spreadsheet_fill_xml)),
    )
    .collect::<Vec<_>>()
    .join("");
    let borders = std::iter::once("<border/>".to_string())
        .chain(
            border_ids
                .iter()
                .filter_map(|id| artifact.borders.get(id).map(spreadsheet_border_xml)),
        )
        .collect::<Vec<_>>()
        .join("");
    let number_formats = number_format_ids
        .iter()
        .filter_map(|id| {
            artifact.number_formats.get(id).and_then(|format| {
                let excel_id = number_format_indices.get(id).copied()?;
                format
                    .format_code
                    .as_ref()
                    .filter(|_| excel_id >= 164)
                    .map(|format_code| {
                        format!(
                            r#"<numFmt numFmtId="{excel_id}" formatCode="{}"/>"#,
                            xml_escape(format_code)
                        )
                    })
            })
        })
        .collect::<Vec<_>>();
    let num_fmts_xml = if number_formats.is_empty() {
        String::new()
    } else {
        format!(
            r#"<numFmts count="{}">{}</numFmts>"#,
            number_formats.len(),
            number_formats.join("")
        )
    };
    let differential_formats = differential_format_ids
        .iter()
        .filter_map(|id| {
            artifact.differential_formats.get(id).map(|format| {
                spreadsheet_differential_format_xml(artifact, format, &number_format_indices)
            })
        })
        .collect::<Vec<_>>()
        .join("");
    let cell_xfs = (0..=max_style_index)
        .map(|style_index| {
            let format = artifact
                .resolve_cell_format(style_index)
                .or_else(|| artifact.get_cell_format(style_index).cloned())
                .unwrap_or_default();
            spreadsheet_cell_format_xml(
                &format,
                &font_indices,
                &fill_indices,
                &border_indices,
                &number_format_indices,
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
            r#"{}"#,
            r#"<fonts count="{}">{}</fonts>"#,
            r#"<fills count="{}">{}</fills>"#,
            r#"<borders count="{}">{}</borders>"#,
            r#"<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>"#,
            r#"<cellXfs count="{}">{}</cellXfs>"#,
            r#"<dxfs count="{}">{}</dxfs>"#,
            r#"<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>"#,
            r#"</styleSheet>"#
        ),
        num_fmts_xml,
        font_ids.len() + 1,
        fonts,
        fill_ids.len() + 2,
        fills,
        border_ids.len() + 1,
        borders,
        max_style_index + 1,
        cell_xfs,
        differential_format_ids.len(),
        differential_formats,
    )
}

fn sheet_xml(sheet: &SpreadsheetSheet) -> String {
    let mut rows = BTreeMap::<u32, Vec<(CellAddress, &SpreadsheetCell)>>::new();
    for row_index in sheet.row_heights.keys() {
        rows.entry(*row_index).or_default();
    }
    for (address, cell) in &sheet.cells {
        rows.entry(address.row).or_default().push((*address, cell));
    }

    let sheet_data = rows
        .into_iter()
        .map(|(row_index, mut entries)| {
            entries.sort_by_key(|(address, _)| address.column);
            let cells = entries
                .into_iter()
                .map(|(address, cell)| cell_xml(address, cell))
                .collect::<Vec<_>>()
                .join("");
            let height = sheet
                .row_heights
                .get(&row_index)
                .map(|value| format!(r#" ht="{value}" customHeight="1""#))
                .unwrap_or_default();
            format!(r#"<row r="{row_index}"{height}>{cells}</row>"#)
        })
        .collect::<Vec<_>>()
        .join("");

    let cols = if sheet.column_widths.is_empty() {
        String::new()
    } else {
        let mut groups = Vec::new();
        let mut iter = sheet.column_widths.iter().peekable();
        while let Some((&start, &width)) = iter.next() {
            let mut end = start;
            while let Some((next_column, next_width)) =
                iter.peek().map(|(column, width)| (**column, **width))
            {
                if next_column == end + 1 && (next_width - width).abs() < f64::EPSILON {
                    end = next_column;
                    iter.next();
                } else {
                    break;
                }
            }
            groups.push(format!(
                r#"<col min="{start}" max="{end}" width="{width}" customWidth="1"/>"#
            ));
        }
        format!("<cols>{}</cols>", groups.join(""))
    };

    let merge_cells = if sheet.merged_ranges.is_empty() {
        String::new()
    } else {
        format!(
            r#"<mergeCells count="{}">{}</mergeCells>"#,
            sheet.merged_ranges.len(),
            sheet
                .merged_ranges
                .iter()
                .map(|range| format!(r#"<mergeCell ref="{}"/>"#, range.to_a1()))
                .collect::<Vec<_>>()
                .join("")
        )
    };

    let default_row_height = sheet.default_row_height.unwrap_or(15.0);
    let default_column_width = sheet.default_column_width.unwrap_or(8.43);
    let grid_lines = if sheet.show_grid_lines { "1" } else { "0" };

    format!(
        "{}{}<sheetViews><sheetView workbookViewId=\"0\" showGridLines=\"{}\"/></sheetViews><sheetFormatPr defaultRowHeight=\"{}\" defaultColWidth=\"{}\"/>{}<sheetData>{}</sheetData>{}{}",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
        grid_lines,
        default_row_height,
        default_column_width,
        cols,
        sheet_data,
        merge_cells,
        r#"</worksheet>"#
    )
}

fn cell_xml(address: CellAddress, cell: &SpreadsheetCell) -> String {
    let style = if cell.style_index == 0 {
        String::new()
    } else {
        format!(r#" s="{}""#, cell.style_index)
    };

    if let Some(formula) = &cell.formula {
        let formula = xml_escape(formula.trim_start_matches('='));
        let value_xml = match &cell.value {
            Some(SpreadsheetCellValue::Bool(value)) => {
                format!(
                    r#" t="b"><f>{formula}</f><v>{}</v></c>"#,
                    usize::from(*value)
                )
            }
            Some(SpreadsheetCellValue::Integer(value)) => {
                format!(r#"><f>{formula}</f><v>{value}</v></c>"#)
            }
            Some(SpreadsheetCellValue::Float(value)) => {
                format!(r#"><f>{formula}</f><v>{value}</v></c>"#)
            }
            Some(SpreadsheetCellValue::String(value))
            | Some(SpreadsheetCellValue::DateTime(value)) => format!(
                r#" t="str"><f>{formula}</f><v>{}</v></c>"#,
                xml_escape(value)
            ),
            Some(SpreadsheetCellValue::Error(value)) => {
                format!(r#" t="e"><f>{formula}</f><v>{}</v></c>"#, xml_escape(value))
            }
            None => format!(r#"><f>{formula}</f></c>"#),
        };
        return format!(r#"<c r="{}"{style}{value_xml}"#, address.to_a1());
    }

    match &cell.value {
        Some(SpreadsheetCellValue::Bool(value)) => format!(
            r#"<c r="{}"{style} t="b"><v>{}</v></c>"#,
            address.to_a1(),
            usize::from(*value)
        ),
        Some(SpreadsheetCellValue::Integer(value)) => {
            format!(r#"<c r="{}"{style}><v>{value}</v></c>"#, address.to_a1())
        }
        Some(SpreadsheetCellValue::Float(value)) => {
            format!(r#"<c r="{}"{style}><v>{value}</v></c>"#, address.to_a1())
        }
        Some(SpreadsheetCellValue::String(value)) | Some(SpreadsheetCellValue::DateTime(value)) => {
            format!(
                r#"<c r="{}"{style} t="inlineStr"><is><t>{}</t></is></c>"#,
                address.to_a1(),
                xml_escape(value)
            )
        }
        Some(SpreadsheetCellValue::Error(value)) => format!(
            r#"<c r="{}"{style} t="e"><v>{}</v></c>"#,
            address.to_a1(),
            xml_escape(value)
        ),
        None => format!(r#"<c r="{}"{style}/>"#, address.to_a1()),
    }
}

fn first_tag_attributes(xml: &str, tag: &str) -> Option<String> {
    let regex = Regex::new(&format!(r#"<{tag}\b([^>]*)/?>"#)).ok()?;
    let captures = regex.captures(xml)?;
    captures.get(1).map(|value| value.as_str().to_string())
}

fn first_tag_attributes_ns(xml: &str, tag: &str) -> Option<String> {
    let regex = Regex::new(&format!(r#"<(?:[A-Za-z0-9_]+:)?{tag}\b([^>]*)/?>"#)).ok()?;
    let captures = regex.captures(xml)?;
    captures.get(1).map(|value| value.as_str().to_string())
}

fn child_tags(xml: &str, tag: &str) -> Result<Vec<(String, String)>, SpreadsheetArtifactError> {
    let regex = Regex::new(&format!(
        r#"(?s)<{tag}\b([^>]*)/>|<{tag}\b([^>]*)>(.*?)</{tag}>"#
    ))
    .map_err(|error| SpreadsheetArtifactError::Serialization {
        message: error.to_string(),
    })?;
    Ok(regex
        .captures_iter(xml)
        .map(|captures| {
            let attributes = captures
                .get(1)
                .or_else(|| captures.get(2))
                .map(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let body = captures
                .get(3)
                .map(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            (attributes, body)
        })
        .collect())
}

fn child_tags_ns(xml: &str, tag: &str) -> Result<Vec<(String, String)>, SpreadsheetArtifactError> {
    let regex = Regex::new(&format!(
        r#"(?s)<(?:[A-Za-z0-9_]+:)?{tag}\b([^>]*)/>|<(?:[A-Za-z0-9_]+:)?{tag}\b([^>]*)>(.*?)</(?:[A-Za-z0-9_]+:)?{tag}>"#
    ))
    .map_err(|error| SpreadsheetArtifactError::Serialization {
        message: error.to_string(),
    })?;
    Ok(regex
        .captures_iter(xml)
        .map(|captures| {
            let attributes = captures
                .get(1)
                .or_else(|| captures.get(2))
                .map(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let body = captures
                .get(3)
                .map(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            (attributes, body)
        })
        .collect())
}

fn first_tag_text(xml: &str, tag: &str) -> Option<String> {
    let regex = Regex::new(&format!(r#"(?s)<{tag}\b[^>]*>(.*?)</{tag}>"#)).ok()?;
    let captures = regex.captures(xml)?;
    captures.get(1).map(|value| value.as_str().to_string())
}

fn first_tag_text_ns(xml: &str, tag: &str) -> Option<String> {
    let regex = Regex::new(&format!(
        r#"(?s)<(?:[A-Za-z0-9_]+:)?{tag}\b[^>]*>(.*?)</(?:[A-Za-z0-9_]+:)?{tag}>"#
    ))
    .ok()?;
    let captures = regex.captures(xml)?;
    captures.get(1).map(|value| value.as_str().to_string())
}

fn extract_workbook_title(xml: &str) -> String {
    let Ok(regex) =
        Regex::new(r#"(?s)<(?:[A-Za-z0-9_]+:)?title\b[^>]*>(.*?)</(?:[A-Za-z0-9_]+:)?title>"#)
    else {
        return String::new();
    };
    regex
        .captures(xml)
        .and_then(|captures| captures.get(1).map(|value| xml_unescape(value.as_str())))
        .unwrap_or_default()
}

fn all_text_nodes(xml: &str) -> Result<String, SpreadsheetArtifactError> {
    let regex = Regex::new(r#"(?s)<t\b[^>]*>(.*?)</t>"#).map_err(|error| {
        SpreadsheetArtifactError::Serialization {
            message: error.to_string(),
        }
    })?;
    Ok(regex
        .captures_iter(xml)
        .filter_map(|captures| captures.get(1).map(|value| xml_unescape(value.as_str())))
        .collect::<Vec<_>>()
        .join(""))
}

fn all_text_nodes_any_ns(xml: &str) -> Result<String, SpreadsheetArtifactError> {
    let regex = Regex::new(r#"(?s)<(?:[A-Za-z0-9_]+:)?t\b[^>]*>(.*?)</(?:[A-Za-z0-9_]+:)?t>"#)
        .map_err(|error| SpreadsheetArtifactError::Serialization {
            message: error.to_string(),
        })?;
    Ok(regex
        .captures_iter(xml)
        .filter_map(|captures| captures.get(1).map(|value| xml_unescape(value.as_str())))
        .collect::<Vec<_>>()
        .join(""))
}

fn extract_attribute(attributes: &str, name: &str) -> Option<String> {
    let pattern = format!(r#"{name}="([^"]*)""#);
    let regex = Regex::new(&pattern).ok()?;
    let captures = regex.captures(attributes)?;
    captures.get(1).map(|value| xml_unescape(value.as_str()))
}

fn parse_color_value(attributes: &str) -> Option<String> {
    let rgb = extract_attribute(attributes, "rgb")?;
    let normalized = rgb.trim();
    if normalized.len() == 8 {
        Some(format!("#{}", normalized[2..].to_ascii_uppercase()))
    } else if normalized.len() == 6 {
        Some(format!("#{}", normalized.to_ascii_uppercase()))
    } else {
        None
    }
}

fn color_xml(tag: &str, color: &str) -> Option<String> {
    let rgb = xlsx_rgb(color)?;
    Some(format!(r#"<{tag} rgb="{rgb}"/>"#))
}

fn xlsx_rgb(color: &str) -> Option<String> {
    let hex = color.trim().trim_start_matches('#');
    if hex.len() == 6 && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(format!("FF{}", hex.to_ascii_uppercase()))
    } else if hex.len() == 8 && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(hex.to_ascii_uppercase())
    } else {
        None
    }
}

fn builtin_number_format_code_for_xlsx(format_id: u32) -> Option<String> {
    match format_id {
        0 => Some("General".to_string()),
        1 => Some("0".to_string()),
        2 => Some("0.00".to_string()),
        3 => Some("#,##0".to_string()),
        4 => Some("#,##0.00".to_string()),
        9 => Some("0%".to_string()),
        10 => Some("0.00%".to_string()),
        _ => None,
    }
}

fn spreadsheet_font_xml(style: &SpreadsheetTextStyle) -> String {
    let mut parts = Vec::new();
    if style.bold == Some(true) {
        parts.push("<b/>".to_string());
    }
    if style.italic == Some(true) {
        parts.push("<i/>".to_string());
    }
    if style.underline == Some(true) {
        parts.push("<u/>".to_string());
    }
    if let Some(font_size) = style.font_size {
        parts.push(format!(r#"<sz val="{font_size}"/>"#));
    }
    if let Some(color) = style
        .font_color
        .as_deref()
        .and_then(|value| color_xml("color", value))
    {
        parts.push(color);
    }
    let font_name = style
        .typeface
        .as_deref()
        .or(style.font_family.as_deref())
        .or_else(|| {
            style
                .font_face
                .as_ref()
                .and_then(|face| face.typeface.as_deref())
        })
        .or_else(|| {
            style
                .font_face
                .as_ref()
                .and_then(|face| face.font_family.as_deref())
        });
    if let Some(font_name) = font_name {
        parts.push(format!(r#"<name val="{}"/>"#, xml_escape(font_name)));
    }
    if let Some(font_scheme) = style.font_scheme.as_deref().or_else(|| {
        style
            .font_face
            .as_ref()
            .and_then(|face| face.font_scheme.as_deref())
    }) {
        parts.push(format!(r#"<scheme val="{}"/>"#, xml_escape(font_scheme)));
    }
    format!("<font>{}</font>", parts.join(""))
}

fn spreadsheet_fill_xml(fill: &SpreadsheetFill) -> String {
    if !fill.gradient_stops.is_empty()
        || fill.gradient_fill_type.is_some()
        || fill.gradient_kind.is_some()
    {
        let gradient_type = fill
            .gradient_kind
            .as_deref()
            .or(fill.gradient_fill_type.as_deref())
            .unwrap_or("linear");
        let gradient_attributes = if gradient_type.eq_ignore_ascii_case("path") {
            let rectangle = fill.fill_rectangle.as_ref();
            format!(
                r#" type="path" left="{}" right="{}" top="{}" bottom="{}""#,
                rectangle.map(|entry| entry.left).unwrap_or(0.0),
                rectangle.map(|entry| entry.right).unwrap_or(0.0),
                rectangle.map(|entry| entry.top).unwrap_or(0.0),
                rectangle.map(|entry| entry.bottom).unwrap_or(0.0)
            )
        } else {
            fill.angle
                .map(|angle| format!(r#" degree="{angle}""#))
                .unwrap_or_default()
        };
        let stops = fill
            .gradient_stops
            .iter()
            .filter_map(|stop| {
                color_xml("color", &stop.color)
                    .map(|color| format!(r#"<stop position="{}">{color}</stop>"#, stop.position))
            })
            .collect::<Vec<_>>()
            .join("");
        return format!(
            r#"<fill><gradientFill{gradient_attributes}>{stops}</gradientFill></fill>"#
        );
    }

    let pattern_type = fill.pattern_type.as_deref().unwrap_or_else(|| {
        if fill.solid_fill_color.is_some() {
            "solid"
        } else {
            "none"
        }
    });
    let fg_color = fill
        .pattern_foreground_color
        .as_deref()
        .or(fill.solid_fill_color.as_deref())
        .and_then(|value| color_xml("fgColor", value))
        .unwrap_or_default();
    let bg_color = fill
        .pattern_background_color
        .as_deref()
        .and_then(|value| color_xml("bgColor", value))
        .unwrap_or_default();
    format!(
        r#"<fill><patternFill patternType="{pattern_type}">{fg_color}{bg_color}</patternFill></fill>"#
    )
}

fn spreadsheet_border_xml(border: &SpreadsheetBorder) -> String {
    format!(
        "<border>{}{}{}{}</border>",
        spreadsheet_border_line_xml("left", border.left.as_ref()),
        spreadsheet_border_line_xml("right", border.right.as_ref()),
        spreadsheet_border_line_xml("top", border.top.as_ref()),
        spreadsheet_border_line_xml("bottom", border.bottom.as_ref()),
    )
}

fn spreadsheet_border_line_xml(edge: &str, line: Option<&SpreadsheetBorderLine>) -> String {
    let Some(line) = line else {
        return format!("<{edge}/>");
    };
    let style = line
        .style
        .as_deref()
        .map(|value| format!(r#" style="{}""#, xml_escape(value)))
        .unwrap_or_default();
    let color = line
        .color
        .as_deref()
        .and_then(|value| color_xml("color", value))
        .unwrap_or_default();
    format!(r#"<{edge}{style}>{color}</{edge}>"#)
}

fn spreadsheet_alignment_xml(
    alignment: Option<&SpreadsheetAlignment>,
    wrap_text: Option<bool>,
) -> String {
    let horizontal = alignment
        .and_then(|alignment| alignment.horizontal.as_deref())
        .map(|value| format!(r#" horizontal="{}""#, xml_escape(value)))
        .unwrap_or_default();
    let vertical = alignment
        .and_then(|alignment| alignment.vertical.as_deref())
        .map(|value| format!(r#" vertical="{}""#, xml_escape(value)))
        .unwrap_or_default();
    let wrap_text = wrap_text
        .map(|value| format!(r#" wrapText="{}""#, usize::from(value)))
        .unwrap_or_default();
    if horizontal.is_empty() && vertical.is_empty() && wrap_text.is_empty() {
        String::new()
    } else {
        format!("<alignment{horizontal}{vertical}{wrap_text}/>")
    }
}

fn spreadsheet_differential_format_xml(
    artifact: &SpreadsheetArtifact,
    format: &crate::SpreadsheetDifferentialFormat,
    number_format_indices: &BTreeMap<u32, u32>,
) -> String {
    let mut parts = Vec::new();
    if let Some(style) = format
        .text_style_id
        .and_then(|id| artifact.text_styles.get(&id))
    {
        parts.push(spreadsheet_font_xml(style));
    }
    if let Some(fill) = format.fill_id.and_then(|id| artifact.fills.get(&id)) {
        parts.push(spreadsheet_fill_xml(fill));
    }
    if let Some(border) = format.border_id.and_then(|id| artifact.borders.get(&id)) {
        parts.push(spreadsheet_border_xml(border));
    }
    if let Some(number_format) = format
        .number_format_id
        .and_then(|id| artifact.number_formats.get(&id))
        && let Some(num_fmt_id) = format
            .number_format_id
            .and_then(|id| number_format_indices.get(&id).copied())
        && let Some(format_code) = number_format.format_code.clone().or_else(|| {
            number_format
                .format_id
                .and_then(builtin_number_format_code_for_xlsx)
        })
    {
        parts.push(format!(
            r#"<numFmt numFmtId="{num_fmt_id}" formatCode="{}"/>"#,
            xml_escape(&format_code)
        ));
    }
    let alignment_xml = spreadsheet_alignment_xml(format.alignment.as_ref(), format.wrap_text);
    if !alignment_xml.is_empty() {
        parts.push(alignment_xml);
    }
    format!("<dxf>{}</dxf>", parts.join(""))
}

fn assign_number_format_ids(
    artifact: &SpreadsheetArtifact,
    number_format_ids: &[u32],
) -> BTreeMap<u32, u32> {
    let mut assigned = BTreeMap::new();
    let mut next_custom_id = 164;
    for id in number_format_ids {
        let Some(format) = artifact.number_formats.get(id) else {
            continue;
        };
        let excel_id = format.format_id.unwrap_or_else(|| {
            while artifact
                .number_formats
                .values()
                .any(|entry| entry.format_id == Some(next_custom_id))
            {
                next_custom_id += 1;
            }
            let assigned_id = next_custom_id;
            next_custom_id += 1;
            assigned_id
        });
        assigned.insert(*id, excel_id);
    }
    assigned
}

fn spreadsheet_cell_format_xml(
    format: &SpreadsheetCellFormat,
    font_indices: &BTreeMap<u32, u32>,
    fill_indices: &BTreeMap<u32, u32>,
    border_indices: &BTreeMap<u32, u32>,
    number_format_indices: &BTreeMap<u32, u32>,
) -> String {
    let font_id = format
        .text_style_id
        .and_then(|id| font_indices.get(&id).copied())
        .unwrap_or(0);
    let fill_id = format
        .fill_id
        .and_then(|id| fill_indices.get(&id).copied())
        .unwrap_or(0);
    let border_id = format
        .border_id
        .and_then(|id| border_indices.get(&id).copied())
        .unwrap_or(0);
    let num_fmt_id = format
        .number_format_id
        .and_then(|id| number_format_indices.get(&id).copied())
        .unwrap_or(0);

    let alignment_xml = spreadsheet_alignment_xml(format.alignment.as_ref(), format.wrap_text);

    let apply_alignment = usize::from(!alignment_xml.is_empty());
    let apply_number_format = usize::from(num_fmt_id != 0);
    let apply_fill = usize::from(fill_id != 0);
    let apply_border = usize::from(border_id != 0);
    let apply_font = usize::from(font_id != 0);

    if alignment_xml.is_empty() {
        format!(
            r#"<xf numFmtId="{num_fmt_id}" fontId="{font_id}" fillId="{fill_id}" borderId="{border_id}" xfId="0" applyNumberFormat="{apply_number_format}" applyFont="{apply_font}" applyFill="{apply_fill}" applyBorder="{apply_border}"/>"#
        )
    } else {
        format!(
            r#"<xf numFmtId="{num_fmt_id}" fontId="{font_id}" fillId="{fill_id}" borderId="{border_id}" xfId="0" applyNumberFormat="{apply_number_format}" applyFont="{apply_font}" applyFill="{apply_fill}" applyBorder="{apply_border}" applyAlignment="{apply_alignment}">{alignment_xml}</xf>"#
        )
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::normalize_relationship_target;
    use super::sheet_relationships_path;

    #[test]
    fn relationship_paths_use_zip_separators() {
        assert_eq!(
            sheet_relationships_path("xl/worksheets/sheet1.xml").unwrap(),
            "xl/worksheets/_rels/sheet1.xml.rels"
        );
        assert_eq!(
            normalize_relationship_target("xl/worksheets/sheet1.xml", "../tables/table1.xml")
                .unwrap(),
            "xl/tables/table1.xml"
        );
        assert_eq!(
            normalize_relationship_target("xl\\drawings\\drawing1.xml", "..\\charts\\chart1.xml")
                .unwrap(),
            "xl/charts/chart1.xml"
        );
    }
}
