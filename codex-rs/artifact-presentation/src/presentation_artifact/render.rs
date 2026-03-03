use font8x8::BASIC_FONTS;
use font8x8::UnicodeFonts;
use image::DynamicImage;
use image::Rgba;
use image::RgbaImage;
use std::f32::consts::PI;
use tiny_skia::FillRule;
use tiny_skia::LineCap;
use tiny_skia::LineJoin;
use tiny_skia::Paint;
use tiny_skia::PathBuilder;
use tiny_skia::Pixmap;
use tiny_skia::Stroke;
use tiny_skia::Path as TinyPath;
use tiny_skia::Transform;

const DEFAULT_BACKGROUND_HEX: &str = "FFFFFF";
const DEFAULT_TEXT_HEX: &str = "000000";
const DEFAULT_LINK_HEX: &str = "1155CC";
const DEFAULT_SHAPE_STROKE_HEX: &str = "666666";
const DEFAULT_TABLE_GRID_HEX: &str = "BFC5CC";
const DEFAULT_COMMENT_HEX: &str = "F3C83C";
const DEFAULT_TEXT_INSET: u32 = 6;
const MIN_TEXT_PIXELS: u32 = 8;

#[derive(Debug, Clone, Copy)]
pub(crate) struct RenderOptions {
    pub scale: f32,
    pub include_background: bool,
}

#[derive(Clone)]
struct StyledGlyph {
    ch: char,
    style: TextStyle,
    is_link: bool,
}

struct TextLine {
    glyphs: Vec<StyledGlyph>,
    width: u32,
    spaces: usize,
}

#[derive(Clone, Copy)]
struct TextRenderConfig {
    width: u32,
    font_px: u32,
    line_height: u32,
    wrap: TextWrapMode,
    vertical_alignment: TextVerticalAlignment,
    alignment: TextAlignment,
}

#[derive(Clone, Copy)]
struct TextBounds {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
}

fn render_slide_png(
    document: &PresentationDocument,
    slide_index: usize,
    options: RenderOptions,
) -> Result<Vec<u8>, PresentationArtifactError> {
    if !options.scale.is_finite() || options.scale <= 0.0 {
        return Err(PresentationArtifactError::RenderFailed {
            action: "render_preview".to_string(),
            message: "`scale` must be a positive number".to_string(),
        });
    }

    let slide = document
        .slides
        .get(slide_index)
        .ok_or_else(|| index_out_of_range("render_preview", slide_index, document.slides.len()))?;
    let width = scaled_dimension(document.slide_size.width, options.scale);
    let height = scaled_dimension(document.slide_size.height, options.scale);
    let mut canvas = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));

    if options.include_background {
        fill_rect(
            &mut canvas,
            0,
            0,
            width,
            height,
            slide_background_rgba(slide, document),
        );
    }

    let mut ordered = slide.elements.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|element| element.z_order());
    for element in ordered {
        render_element(&mut canvas, document, slide, element, options.scale)?;
    }

    render_comment_overlays(&mut canvas, document, slide, options.scale);

    let mut output = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(canvas)
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|error| PresentationArtifactError::RenderFailed {
            action: "render_preview".to_string(),
            message: error.to_string(),
        })?;
    Ok(output.into_inner())
}

fn render_element(
    canvas: &mut RgbaImage,
    document: &PresentationDocument,
    slide: &PresentationSlide,
    element: &PresentationElement,
    scale: f32,
) -> Result<(), PresentationArtifactError> {
    match element {
        PresentationElement::Text(text) => {
            let frame = scaled_rect(text.frame, scale);
            let mut image =
                RgbaImage::from_pixel(frame.width.max(1), frame.height.max(1), Rgba([0, 0, 0, 0]));
            if let Some(fill) = &text.fill {
                let image_width = image.width();
                let image_height = image.height();
                fill_rect(&mut image, 0, 0, image_width, image_height, parse_rgba(fill, 255));
            }
            draw_text_box(
                &mut image,
                &text.text,
                &text.style,
                &text.rich_text,
                text.hyperlink.as_ref(),
                scale,
            );
            blend_image(canvas, &image, frame.left, frame.top);
        }
        PresentationElement::Shape(shape) => {
            let frame = scaled_rect(shape.frame, scale);
            let mut image =
                RgbaImage::from_pixel(frame.width.max(1), frame.height.max(1), Rgba([0, 0, 0, 0]));
            let vector = render_vector_image(image.width(), image.height(), |pixmap| {
                render_shape_vector(pixmap, shape, scale)
            })?;
            blend_image(&mut image, &vector, 0, 0);
            if let Some(text) = &shape.text {
                let mut text_layer = RgbaImage::from_pixel(
                    image.width(),
                    image.height(),
                    Rgba([0, 0, 0, 0]),
                );
                let rich_text = shape.rich_text.clone().unwrap_or_default();
                draw_text_box(
                    &mut text_layer,
                    text,
                    &shape.text_style,
                    &rich_text,
                    shape.hyperlink.as_ref(),
                    scale,
                );
                blend_image(&mut image, &text_layer, 0, 0);
            }
            blend_transformed_image(
                canvas,
                &image,
                frame.left,
                frame.top,
                shape.rotation_degrees.unwrap_or(0) as f32,
                shape.flip_horizontal,
                shape.flip_vertical,
            );
        }
        PresentationElement::Connector(connector) => {
            let (left, top, width, height) = scaled_connector_bounds(connector, scale);
            let image = render_vector_image(width.max(1), height.max(1), |pixmap| {
                render_connector_vector(pixmap, connector, scale)
            })?;
            blend_image(canvas, &image, left, top);
            if let Some(label) = &connector.label {
                let mut label_image = RgbaImage::from_pixel(
                    width.max(1),
                    height.max(1),
                    Rgba([0, 0, 0, 0]),
                );
                draw_text_box(
                    &mut label_image,
                    label,
                    &TextStyle {
                        font_size: Some(12),
                        color: Some(DEFAULT_TEXT_HEX.to_string()),
                        ..TextStyle::default()
                    },
                    &RichTextState::default(),
                    None,
                    scale,
                );
                blend_image(canvas, &label_image, left, top);
            }
        }
        PresentationElement::Image(image) => {
            let frame = scaled_rect(image.frame, scale);
            let rendered = render_image_element(image, scale)?;
            blend_transformed_image(
                canvas,
                &rendered,
                frame.left,
                frame.top,
                image.rotation_degrees.unwrap_or(0) as f32,
                image.flip_horizontal,
                image.flip_vertical,
            );
        }
        PresentationElement::Table(table) => {
            let frame = scaled_rect(table.frame, scale);
            let rendered = render_table_element(table, document, scale)?;
            blend_image(canvas, &rendered, frame.left, frame.top);
        }
        PresentationElement::Chart(chart) => {
            let frame = scaled_rect(chart.frame, scale);
            let rendered = render_chart_element(chart, document, scale)?;
            blend_image(canvas, &rendered, frame.left, frame.top);
        }
    }

    let _ = slide;
    Ok(())
}

fn render_shape_vector(
    pixmap: &mut Pixmap,
    shape: &ShapeElement,
    scale: f32,
) -> Result<(), PresentationArtifactError> {
    let path = shape_path(shape.geometry, pixmap.width(), pixmap.height())?;
    if let Some(fill) = &shape.fill {
        let mut paint = Paint::default();
        set_paint_color(&mut paint, parse_rgba(fill, 255));
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
    if let Some(stroke) = &shape.stroke {
        let mut paint = Paint::default();
        set_paint_color(&mut paint, parse_rgba(&stroke.color, 255));
        let style = Stroke {
            width: (stroke.width.max(1) as f32 * scale).max(1.0),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &style, Transform::identity(), None);
    } else if shape.fill.is_none() {
        let mut paint = Paint::default();
        set_paint_color(&mut paint, parse_rgba(DEFAULT_SHAPE_STROKE_HEX, 255));
        let style = Stroke {
            width: scale.max(1.0),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &style, Transform::identity(), None);
    }
    Ok(())
}

fn render_connector_vector(
    pixmap: &mut Pixmap,
    connector: &ConnectorElement,
    scale: f32,
) -> Result<(), PresentationArtifactError> {
    let (left, top, width, height) = scaled_connector_bounds(connector, scale);
    let start = (
        (connector.start.left as f32 * scale) - left as f32,
        (connector.start.top as f32 * scale) - top as f32,
    );
    let end = (
        (connector.end.left as f32 * scale) - left as f32,
        (connector.end.top as f32 * scale) - top as f32,
    );
    let path = connector_path(connector.connector_type, start, end)?;
    let mut paint = Paint::default();
    set_paint_color(&mut paint, parse_rgba(&connector.line.color, 255));
    let stroke = Stroke {
        width: (connector.line.width.max(1) as f32 * scale).max(1.0),
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);

    let arrow_px = match connector.arrow_size {
        ConnectorArrowScale::Small => 8.0 * scale,
        ConnectorArrowScale::Medium => 12.0 * scale,
        ConnectorArrowScale::Large => 16.0 * scale,
    };
    draw_arrow_head(pixmap, end, start, connector.end_arrow, arrow_px, &connector.line.color)?;
    draw_arrow_head(
        pixmap,
        start,
        end,
        connector.start_arrow,
        arrow_px,
        &connector.line.color,
    )?;
    let _ = width;
    let _ = height;
    Ok(())
}

fn render_image_element(
    image: &ImageElement,
    scale: f32,
) -> Result<RgbaImage, PresentationArtifactError> {
    let frame = scaled_rect(image.frame, scale);
    let mut canvas = RgbaImage::from_pixel(frame.width.max(1), frame.height.max(1), Rgba([0, 0, 0, 0]));
    let Some(payload) = &image.payload else {
        let border = render_vector_image(canvas.width(), canvas.height(), |pixmap| {
            let path = shape_path(ShapeGeometry::Rectangle, pixmap.width(), pixmap.height())?;
            let mut paint = Paint::default();
            set_paint_color(&mut paint, parse_rgba(DEFAULT_SHAPE_STROKE_HEX, 255));
            let stroke = Stroke {
                width: scale.max(1.0),
                ..Stroke::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            Ok(())
        })?;
        blend_image(&mut canvas, &border, 0, 0);
        draw_text_box(
            &mut canvas,
            image.prompt.as_deref().unwrap_or("Image placeholder"),
            &TextStyle {
                font_size: Some(14),
                color: Some(DEFAULT_TEXT_HEX.to_string()),
                alignment: Some(TextAlignment::Center),
                ..TextStyle::default()
            },
            &RichTextState {
                layout: TextLayoutState {
                    vertical_alignment: Some(TextVerticalAlignment::Middle),
                    ..TextLayoutState::default()
                },
                ..RichTextState::default()
            },
            None,
            scale,
        );
        return Ok(canvas);
    };

    let mut source =
        image::load_from_memory(&payload.bytes).map_err(|error| PresentationArtifactError::RenderFailed {
            action: "render_preview".to_string(),
            message: format!("failed to decode image `{}`: {error}", image.element_id),
        })?;
    if let Some((left, top, right, bottom)) = image.crop {
        source = crop_dynamic_image(source, left, top, right, bottom);
    }
    let (left, top, width, height, cover_crop) = fit_image(image);
    let target_left = ((left - image.frame.left) as f32 * scale).round() as u32;
    let target_top = ((top - image.frame.top) as f32 * scale).round() as u32;
    let target_width = scaled_dimension(width, scale);
    let target_height = scaled_dimension(height, scale);
    if let Some((crop_left, crop_top, crop_right, crop_bottom)) = cover_crop {
        source = crop_dynamic_image(source, crop_left, crop_top, crop_right, crop_bottom);
    }
    let resized = source.resize_exact(
        target_width.max(1),
        target_height.max(1),
        image::imageops::FilterType::Lanczos3,
    );
    let rgba = resized.to_rgba8();
    blend_image(&mut canvas, &rgba, target_left, target_top);
    Ok(canvas)
}

fn render_table_element(
    table: &TableElement,
    document: &PresentationDocument,
    scale: f32,
) -> Result<RgbaImage, PresentationArtifactError> {
    let frame = scaled_rect(table.frame, scale);
    let mut image = RgbaImage::from_pixel(frame.width.max(1), frame.height.max(1), Rgba([0, 0, 0, 0]));
    let row_heights = scaled_lengths(&table.row_heights, scale);
    let column_widths = scaled_lengths(&table.column_widths, scale);
    let outside = table
        .borders
        .as_ref()
        .and_then(|borders| borders.outside.as_ref())
        .map(|border| (parse_rgba(&border.color, 255), scaled_dimension(border.width, scale)))
        .unwrap_or((parse_rgba(DEFAULT_TABLE_GRID_HEX, 255), 1));
    let inside = table
        .borders
        .as_ref()
        .and_then(|borders| borders.inside.as_ref())
        .map(|border| (parse_rgba(&border.color, 255), scaled_dimension(border.width, scale)))
        .unwrap_or((parse_rgba(DEFAULT_TABLE_GRID_HEX, 255), 1));
    let merge_map = build_table_merge_lookup(&table.merges);
    let mut top = 0;
    for (row_index, row) in table.rows.iter().enumerate() {
        let row_height = row_heights
            .get(row_index)
            .copied()
            .unwrap_or_else(|| scaled_dimension(24, scale));
        let mut left = 0;
        for (column_index, cell) in row.iter().enumerate() {
            let column_width = column_widths
                .get(column_index)
                .copied()
                .unwrap_or_else(|| scaled_dimension(80, scale));
            if is_merged_away(&merge_map, row_index, column_index) {
                left += column_width;
                continue;
            }
            let (span_rows, span_columns) =
                merged_span(&merge_map, row_index, column_index).unwrap_or((1, 1));
            let cell_width = column_widths
                .iter()
                .skip(column_index)
                .take(span_columns)
                .copied()
                .sum::<u32>();
            let cell_height = row_heights
                .iter()
                .skip(row_index)
                .take(span_rows)
                .copied()
                .sum::<u32>();
            let fill = cell
                .background_fill
                .as_deref()
                .map(|value| parse_rgba(value, 255))
                .unwrap_or_else(|| table_cell_fill(table, row_index, column_index));
            fill_rect(&mut image, left, top, cell_width, cell_height, fill);
            draw_rect_border(&mut image, left, top, cell_width, cell_height, outside.0, outside.1);
            let alignment = cell.alignment.or(cell.text_style.alignment).unwrap_or(TextAlignment::Left);
            draw_text_in_bounds(
                &mut image,
                TextBounds {
                    left,
                    top,
                    width: cell_width,
                    height: cell_height,
                },
                &cell.text,
                &TextStyle {
                    color: cell
                        .text_style
                        .color
                        .clone()
                        .or_else(|| document.theme.resolve_color("tx1")),
                    alignment: Some(alignment),
                    ..cell.text_style.clone()
                },
                &cell.rich_text,
                None,
                scale,
            );
            left += column_width;
        }
        top += row_height;
    }

    let mut y = 0;
    for height in row_heights.iter().take(row_heights.len().saturating_sub(1)) {
        y += *height;
        draw_horizontal_line(&mut image, y, inside.0, inside.1);
    }
    let mut x = 0;
    for width in column_widths
        .iter()
        .take(column_widths.len().saturating_sub(1))
    {
        x += *width;
        draw_vertical_line(&mut image, x, inside.0, inside.1);
    }
    Ok(image)
}

fn render_chart_element(
    chart: &ChartElement,
    document: &PresentationDocument,
    scale: f32,
) -> Result<RgbaImage, PresentationArtifactError> {
    let frame = scaled_rect(chart.frame, scale);
    let mut image = RgbaImage::from_pixel(frame.width.max(1), frame.height.max(1), Rgba([0, 0, 0, 0]));
    let image_width = image.width();
    let image_height = image.height();
    let chart_fill = chart
        .chart_fill
        .as_deref()
        .map(|value| parse_rgba(value, 255))
        .unwrap_or(Rgba([255, 255, 255, 255]));
    fill_rect(&mut image, 0, 0, image_width, image_height, chart_fill);
    draw_rect_border(
        &mut image,
        0,
        0,
        image_width,
        image_height,
        parse_rgba(DEFAULT_TABLE_GRID_HEX, 255),
        1,
    );

    let title_height = scaled_dimension(28, scale).min(image.height());
    if let Some(title) = &chart.title {
        draw_text_in_bounds(
            &mut image,
            TextBounds {
                left: 8,
                top: 4,
                width: image_width.saturating_sub(16),
                height: title_height.saturating_sub(4),
            },
            title,
            &TextStyle {
                font_size: Some(16),
                bold: true,
                color: document.theme.resolve_color("tx1"),
                alignment: Some(TextAlignment::Center),
                ..TextStyle::default()
            },
            &RichTextState::default(),
            None,
            scale,
        );
    }

    let legend_height = if chart.has_legend && !chart.series.is_empty() {
        scaled_dimension(22, scale)
    } else {
        0
    };
    let plot_left = scaled_dimension(36, scale).min(image.width());
    let plot_top = title_height + scaled_dimension(8, scale);
    let plot_width = image
        .width()
        .saturating_sub(plot_left + scaled_dimension(16, scale));
    let plot_height = image
        .height()
        .saturating_sub(plot_top + legend_height + scaled_dimension(12, scale));
    if plot_width == 0 || plot_height == 0 {
        return Ok(image);
    }

    match chart.chart_type {
        ChartTypeSpec::Bar
        | ChartTypeSpec::BarStacked
        | ChartTypeSpec::BarStacked100
        | ChartTypeSpec::BarHorizontal => render_bar_chart(
            &mut image,
            chart,
            plot_left,
            plot_top,
            plot_width,
            plot_height,
            scale,
        ),
        ChartTypeSpec::Line
        | ChartTypeSpec::LineMarkers
        | ChartTypeSpec::LineStacked
        | ChartTypeSpec::Area
        | ChartTypeSpec::AreaStacked
        | ChartTypeSpec::AreaStacked100
        | ChartTypeSpec::Scatter
        | ChartTypeSpec::ScatterLines
        | ChartTypeSpec::ScatterSmooth
        | ChartTypeSpec::Bubble => render_line_chart(
            &mut image,
            chart,
            plot_left,
            plot_top,
            plot_width,
            plot_height,
            scale,
        ),
        ChartTypeSpec::Pie | ChartTypeSpec::Doughnut => {
            render_pie_chart(&mut image, chart, plot_left, plot_top, plot_width, plot_height)
        }
        _ => {
            draw_text_in_bounds(
                &mut image,
                TextBounds {
                    left: plot_left,
                    top: plot_top,
                    width: plot_width,
                    height: plot_height,
                },
                "Preview uses a simplified chart placeholder for this chart type.",
                &TextStyle {
                    font_size: Some(12),
                    color: Some(DEFAULT_TEXT_HEX.to_string()),
                    alignment: Some(TextAlignment::Center),
                    ..TextStyle::default()
                },
                &RichTextState {
                    layout: TextLayoutState {
                        vertical_alignment: Some(TextVerticalAlignment::Middle),
                        ..TextLayoutState::default()
                    },
                    ..RichTextState::default()
                },
                None,
                scale,
            );
        }
    }

    if legend_height > 0 {
        let legend_top = image.height().saturating_sub(legend_height + 4);
        render_chart_legend(
            &mut image,
            chart,
            8,
            legend_top,
            image_width.saturating_sub(16),
            legend_height,
            scale,
        );
    }
    Ok(image)
}

fn render_bar_chart(
    image: &mut RgbaImage,
    chart: &ChartElement,
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    scale: f32,
) {
    let baseline = top + height;
    draw_chart_axes(image, left, top, width, height);
    let series_count = chart.series.len().max(1) as u32;
    let category_count = chart.categories.len().max(1) as u32;
    let max_value = chart
        .series
        .iter()
        .flat_map(|series| series.values.iter().copied())
        .fold(0.0f64, f64::max)
        .max(1.0);
    let group_width = width as f32 / category_count as f32;
    let bar_width = (group_width / series_count as f32 * 0.72).max(2.0);
    for (series_index, series) in chart.series.iter().enumerate() {
        let color = chart_series_color(series, series_index);
        for (value_index, value) in series.values.iter().enumerate() {
            let bar_height = ((*value / max_value) as f32 * height as f32).round().max(0.0) as u32;
            let x = left as f32
                + group_width * value_index as f32
                + (group_width - bar_width * series_count as f32) / 2.0
                + bar_width * series_index as f32;
            let y = baseline.saturating_sub(bar_height);
            fill_rect(
                image,
                x.round() as u32,
                y,
                bar_width.round().max(1.0) as u32,
                bar_height.max(1),
                color,
            );
        }
    }
    render_category_labels(image, &chart.categories, left, baseline + 4, width, scale);
}

fn render_line_chart(
    image: &mut RgbaImage,
    chart: &ChartElement,
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    scale: f32,
) {
    draw_chart_axes(image, left, top, width, height);
    let baseline = top + height;
    let point_count = chart.categories.len().max(2);
    let max_value = chart
        .series
        .iter()
        .flat_map(|series| series.values.iter().copied())
        .fold(0.0f64, f64::max)
        .max(1.0);
    for (series_index, series) in chart.series.iter().enumerate() {
        let color = chart_series_color(series, series_index);
        let mut points = Vec::new();
        for (value_index, value) in series.values.iter().enumerate() {
            let x = left as f32 + width as f32 * value_index as f32 / (point_count - 1) as f32;
            let y = baseline as f32 - ((*value / max_value) as f32 * height as f32);
            points.push((x, y));
        }
        if points.len() >= 2
            && let Ok(path) = polyline_path(&points)
            && let Ok(line) = render_vector_image(image.width(), image.height(), |pixmap| {
                let mut paint = Paint::default();
                set_paint_color(&mut paint, color);
                let stroke = Stroke {
                    width: scale.max(1.0) * 2.0,
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Stroke::default()
                };
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                Ok(())
            })
        {
            blend_image(image, &line, 0, 0);
        }
        for (x, y) in points {
            fill_rect(
                image,
                x.round().max(2.0) as u32 - 2,
                y.round().max(2.0) as u32 - 2,
                4,
                4,
                color,
            );
        }
    }
    render_category_labels(image, &chart.categories, left, baseline + 4, width, scale);
}

fn render_pie_chart(
    image: &mut RgbaImage,
    chart: &ChartElement,
    left: u32,
    top: u32,
    width: u32,
    height: u32,
) {
    let series = chart.series.first();
    let Some(series) = series else {
        return;
    };
    let radius = (width.min(height) as f32 / 2.0).max(2.0);
    let center = (left as f32 + width as f32 / 2.0, top as f32 + height as f32 / 2.0);
    let total = series
        .values
        .iter()
        .map(|value| value.abs())
        .sum::<f64>()
        .max(1.0);
    let mut start_angle = -PI / 2.0;
    for (index, value) in series.values.iter().enumerate() {
        let sweep = (*value / total) as f32 * PI * 2.0;
        if sweep <= 0.0 {
            continue;
        }
        let end_angle = start_angle + sweep;
        if let Ok(path) = sector_path(center, radius, start_angle, end_angle, chart.chart_type == ChartTypeSpec::Doughnut)
            && let Ok(sector) = render_vector_image(image.width(), image.height(), |pixmap| {
                let mut paint = Paint::default();
                set_paint_color(&mut paint, chart_palette_color(index));
                pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                Ok(())
            })
        {
            blend_image(image, &sector, 0, 0);
        }
        start_angle = end_angle;
    }
}

fn render_chart_legend(
    image: &mut RgbaImage,
    chart: &ChartElement,
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    scale: f32,
) {
    let mut cursor = left;
    let swatch = scaled_dimension(10, scale).max(6);
    for (index, series) in chart.series.iter().enumerate() {
        if cursor >= left + width {
            break;
        }
        fill_rect(image, cursor, top + (height.saturating_sub(swatch)) / 2, swatch, swatch, chart_series_color(series, index));
        cursor += swatch + scaled_dimension(4, scale);
        let label_width = scaled_dimension((series.name.chars().count() as u32).saturating_mul(7), scale).max(40);
        draw_text_in_bounds(
            image,
            TextBounds {
                left: cursor,
                top,
                width: label_width.min(left + width - cursor),
                height,
            },
            &series.name,
            &TextStyle {
                font_size: Some(12),
                color: Some(DEFAULT_TEXT_HEX.to_string()),
                ..TextStyle::default()
            },
            &RichTextState {
                layout: TextLayoutState {
                    vertical_alignment: Some(TextVerticalAlignment::Middle),
                    ..TextLayoutState::default()
                },
                ..RichTextState::default()
            },
            None,
            scale,
        );
        cursor += label_width + scaled_dimension(10, scale);
    }
}

fn render_comment_overlays(
    canvas: &mut RgbaImage,
    document: &PresentationDocument,
    slide: &PresentationSlide,
    scale: f32,
) {
    let radius = scaled_dimension(8, scale).max(6);
    for thread in &document.comment_threads {
        let matches_slide = match &thread.target {
            CommentTarget::Slide { slide_id }
            | CommentTarget::Element { slide_id, .. }
            | CommentTarget::TextRange { slide_id, .. } => slide_id == &slide.slide_id,
        };
        if !matches_slide {
            continue;
        }
        let Some(position) = thread.position.as_ref() else {
            continue;
        };
        let x = scaled_dimension(position.x, scale);
        let y = scaled_dimension(position.y, scale);
        if let Ok(marker) = render_vector_image(radius * 2, radius * 2, |pixmap| {
            let path = ellipse_path(pixmap.width(), pixmap.height())?;
            let mut paint = Paint::default();
            set_paint_color(&mut paint, parse_rgba(DEFAULT_COMMENT_HEX, 230));
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
            Ok(())
        }) {
            blend_image(canvas, &marker, x.saturating_sub(radius), y.saturating_sub(radius));
        }
    }
}

fn render_vector_image(
    width: u32,
    height: u32,
    draw: impl FnOnce(&mut Pixmap) -> Result<(), PresentationArtifactError>,
) -> Result<RgbaImage, PresentationArtifactError> {
    let mut pixmap = Pixmap::new(width.max(1), height.max(1)).ok_or_else(|| {
        PresentationArtifactError::RenderFailed {
            action: "render_preview".to_string(),
            message: format!("failed to allocate pixmap {width}x{height}"),
        }
    })?;
    draw(&mut pixmap)?;
    RgbaImage::from_raw(pixmap.width(), pixmap.height(), pixmap.data().to_vec()).ok_or_else(
        || PresentationArtifactError::RenderFailed {
            action: "render_preview".to_string(),
            message: "failed to convert vector preview".to_string(),
        },
    )
}

fn draw_text_box(
    image: &mut RgbaImage,
    text: &str,
    style: &TextStyle,
    rich_text: &RichTextState,
    hyperlink: Option<&HyperlinkState>,
    scale: f32,
) {
    draw_text_in_bounds(
        image,
        TextBounds {
            left: 0,
            top: 0,
            width: image.width(),
            height: image.height(),
        },
        text,
        style,
        rich_text,
        hyperlink,
        scale,
    );
}

fn draw_text_in_bounds(
    image: &mut RgbaImage,
    bounds: TextBounds,
    text: &str,
    style: &TextStyle,
    rich_text: &RichTextState,
    hyperlink: Option<&HyperlinkState>,
    scale: f32,
) {
    if text.is_empty() || bounds.width == 0 || bounds.height == 0 {
        return;
    }
    let insets = rich_text.layout.insets.unwrap_or(TextInsets {
        left: DEFAULT_TEXT_INSET,
        right: DEFAULT_TEXT_INSET,
        top: DEFAULT_TEXT_INSET,
        bottom: DEFAULT_TEXT_INSET,
    });
    let content_left = bounds.left.saturating_add(scaled_dimension(insets.left, scale));
    let content_top = bounds.top.saturating_add(scaled_dimension(insets.top, scale));
    let content_width = bounds.width
        .saturating_sub(scaled_dimension(insets.left.saturating_add(insets.right), scale));
    let content_height = bounds.height
        .saturating_sub(scaled_dimension(insets.top.saturating_add(insets.bottom), scale));
    if content_width == 0 || content_height == 0 {
        return;
    }

    let glyphs = styled_glyphs(text, style, rich_text, hyperlink);
    let alignment = style.alignment.unwrap_or(TextAlignment::Left);
    let wrap = rich_text.layout.wrap.unwrap_or(TextWrapMode::Square);
    let vertical_alignment = rich_text
        .layout
        .vertical_alignment
        .unwrap_or(TextVerticalAlignment::Top);
    let mut font_px = scaled_dimension(style.font_size.unwrap_or(14), scale).max(MIN_TEXT_PIXELS);
    let auto_fit = rich_text.layout.auto_fit.unwrap_or(TextAutoFitMode::None);
    let config = loop {
        let line_height = ((font_px as f32) * 1.35).round().max(font_px as f32) as u32;
        let config = TextRenderConfig {
            width: content_width,
            font_px,
            line_height,
            wrap,
            vertical_alignment,
            alignment,
        };
        let lines = layout_text_lines(&glyphs, config);
        let used_height = (lines.len() as u32).saturating_mul(line_height);
        let fits_height = used_height <= content_height;
        let fits_width = lines.iter().all(|line| line.width <= content_width);
        if auto_fit != TextAutoFitMode::ShrinkText
            || (fits_height && fits_width)
            || font_px <= MIN_TEXT_PIXELS
        {
            break (config, lines);
        }
        font_px = font_px.saturating_sub(1);
    };

    let (config, lines) = config;
    let used_height = (lines.len() as u32).saturating_mul(config.line_height);
    let mut y = match config.vertical_alignment {
        TextVerticalAlignment::Top => content_top,
        TextVerticalAlignment::Middle => {
            content_top + content_height.saturating_sub(used_height) / 2
        }
        TextVerticalAlignment::Bottom => content_top + content_height.saturating_sub(used_height),
    };

    for (line_index, line) in lines.iter().enumerate() {
        let extra_space = content_width.saturating_sub(line.width);
        let start_x = match config.alignment {
            TextAlignment::Left | TextAlignment::Justify => content_left,
            TextAlignment::Center => content_left + extra_space / 2,
            TextAlignment::Right => content_left + extra_space,
        };
        let justify_gap = if config.alignment == TextAlignment::Justify
            && line_index + 1 < lines.len()
            && line.spaces > 0
        {
            extra_space / line.spaces as u32
        } else {
            0
        };

        let mut x = start_x;
        for glyph in &line.glyphs {
            let glyph_width = draw_bitmap_glyph(image, x, y, glyph, config.font_px);
            x = x.saturating_add(glyph_width);
            if glyph.ch == ' ' && justify_gap > 0 {
                x = x.saturating_add(justify_gap);
            }
        }
        y = y.saturating_add(config.line_height);
        if y >= content_top + content_height {
            break;
        }
    }
}

fn styled_glyphs(
    text: &str,
    base_style: &TextStyle,
    rich_text: &RichTextState,
    hyperlink: Option<&HyperlinkState>,
) -> Vec<StyledGlyph> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut glyphs = chars
        .iter()
        .map(|ch| StyledGlyph {
            ch: *ch,
            style: base_style.clone(),
            is_link: hyperlink.is_some(),
        })
        .collect::<Vec<_>>();
    for range in &rich_text.ranges {
        let glyph_count = glyphs.len();
        let start = range.start_cp.min(glyph_count);
        let end = range.start_cp.saturating_add(range.length).min(glyphs.len());
        for glyph in glyphs
            .iter_mut()
            .skip(start)
            .take(end.saturating_sub(start))
        {
            glyph.style = merged_text_style(&glyph.style, &range.style);
            if range.hyperlink.is_some() {
                glyph.is_link = true;
            }
        }
    }
    if hyperlink.is_some() {
        for glyph in &mut glyphs {
            glyph.is_link = true;
        }
    }
    glyphs
}

fn merged_text_style(base: &TextStyle, overlay: &TextStyle) -> TextStyle {
    TextStyle {
        style_name: overlay.style_name.clone().or(base.style_name.clone()),
        font_size: overlay.font_size.or(base.font_size),
        font_family: overlay.font_family.clone().or(base.font_family.clone()),
        color: overlay.color.clone().or(base.color.clone()),
        alignment: overlay.alignment.or(base.alignment),
        bold: overlay.bold || base.bold,
        italic: overlay.italic || base.italic,
        underline: overlay.underline || base.underline,
    }
}

fn layout_text_lines(glyphs: &[StyledGlyph], config: TextRenderConfig) -> Vec<TextLine> {
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut current_width = 0;
    let mut current_spaces = 0;
    for glyph in glyphs {
        if glyph.ch == '\n' {
            lines.push(TextLine {
                glyphs: std::mem::take(&mut current),
                width: current_width,
                spaces: current_spaces,
            });
            current_width = 0;
            current_spaces = 0;
            continue;
        }
        let glyph_width = measure_glyph_width(glyph, config.font_px);
        if config.wrap == TextWrapMode::Square
            && !current.is_empty()
            && current_width.saturating_add(glyph_width) > config.width
        {
            lines.push(TextLine {
                glyphs: std::mem::take(&mut current),
                width: current_width,
                spaces: current_spaces,
            });
            current_width = 0;
            current_spaces = 0;
        }
        if glyph.ch == ' ' {
            current_spaces += 1;
        }
        current_width = current_width.saturating_add(glyph_width);
        current.push(glyph.clone());
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(TextLine {
            glyphs: current,
            width: current_width,
            spaces: current_spaces,
        });
    }
    lines
}

fn draw_bitmap_glyph(
    image: &mut RgbaImage,
    left: u32,
    top: u32,
    glyph: &StyledGlyph,
    font_px: u32,
) -> u32 {
    let scale = (font_px / 8).max(1);
    if glyph.ch == ' ' {
        return 4 * scale;
    }
    let rows = BASIC_FONTS
        .get(glyph.ch)
        .or_else(|| BASIC_FONTS.get('?'))
        .unwrap_or([0; 8]);
    let color = glyph_color(&glyph.style, glyph.is_link);
    let italic_shift = if glyph.style.italic { scale / 2 } else { 0 };
    for (row_index, row) in rows.iter().enumerate() {
        for column in 0..8u32 {
            if row & (1 << column) == 0 {
                continue;
            }
            let draw_x = left
                .saturating_add((7 - column) * scale)
                .saturating_add(((7 - row_index as u32) * italic_shift) / 6);
            let draw_y = top.saturating_add(row_index as u32 * scale);
            fill_rect(image, draw_x, draw_y, scale, scale, color);
            if glyph.style.bold {
                fill_rect(image, draw_x.saturating_add(1), draw_y, scale, scale, color);
            }
        }
    }
    let width = measure_glyph_width(glyph, font_px);
    if glyph.style.underline || glyph.is_link {
        let underline_y = top.saturating_add(8 * scale).saturating_sub(scale / 2 + 1);
        fill_rect(image, left, underline_y, width.saturating_sub(scale / 2), scale.max(1), color);
    }
    width
}

fn measure_glyph_width(glyph: &StyledGlyph, font_px: u32) -> u32 {
    let scale = (font_px / 8).max(1);
    match glyph.ch {
        ' ' => 4 * scale,
        '\t' => 8 * scale,
        _ => 8 * scale,
    }
}

fn glyph_color(style: &TextStyle, is_link: bool) -> Rgba<u8> {
    if let Some(color) = &style.color {
        return parse_rgba(color, 255);
    }
    if is_link || style.underline {
        parse_rgba(DEFAULT_LINK_HEX, 255)
    } else {
        parse_rgba(DEFAULT_TEXT_HEX, 255)
    }
}

fn slide_background_rgba(slide: &PresentationSlide, document: &PresentationDocument) -> Rgba<u8> {
    slide
        .background_fill
        .as_deref()
        .map(|fill| parse_rgba(fill, 255))
        .or_else(|| {
            document
                .theme
                .resolve_color("bg1")
                .map(|fill| parse_rgba(&fill, 255))
        })
        .unwrap_or_else(|| parse_rgba(DEFAULT_BACKGROUND_HEX, 255))
}

fn scaled_rect(rect: Rect, scale: f32) -> Rect {
    Rect {
        left: scaled_dimension(rect.left, scale),
        top: scaled_dimension(rect.top, scale),
        width: scaled_dimension(rect.width, scale),
        height: scaled_dimension(rect.height, scale),
    }
}

fn scaled_dimension(value: u32, scale: f32) -> u32 {
    ((value as f32) * scale).round().max(1.0) as u32
}

fn scaled_lengths(values: &[u32], scale: f32) -> Vec<u32> {
    values
        .iter()
        .copied()
        .map(|value| scaled_dimension(value, scale))
        .collect()
}

fn scaled_connector_bounds(connector: &ConnectorElement, scale: f32) -> (u32, u32, u32, u32) {
    let min_left = connector.start.left.min(connector.end.left);
    let min_top = connector.start.top.min(connector.end.top);
    let max_left = connector.start.left.max(connector.end.left);
    let max_top = connector.start.top.max(connector.end.top);
    let padding = (connector.line.width.max(8) * 2) as f32 * scale;
    let left = ((min_left as f32) * scale - padding).max(0.0).round() as u32;
    let top = ((min_top as f32) * scale - padding).max(0.0).round() as u32;
    let right = ((max_left as f32) * scale + padding).round() as u32;
    let bottom = ((max_top as f32) * scale + padding).round() as u32;
    (left, top, right.saturating_sub(left), bottom.saturating_sub(top))
}

fn fill_rect(
    image: &mut RgbaImage,
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    color: Rgba<u8>,
) {
    let max_x = left.saturating_add(width).min(image.width());
    let max_y = top.saturating_add(height).min(image.height());
    for y in top.min(image.height())..max_y {
        for x in left.min(image.width())..max_x {
            blend_pixel(image.get_pixel_mut(x, y), color);
        }
    }
}

fn blend_image(canvas: &mut RgbaImage, overlay: &RgbaImage, left: u32, top: u32) {
    for (x, y, pixel) in overlay.enumerate_pixels() {
        if pixel.0[3] == 0 {
            continue;
        }
        let dest_x = left.saturating_add(x);
        let dest_y = top.saturating_add(y);
        if dest_x >= canvas.width() || dest_y >= canvas.height() {
            continue;
        }
        blend_pixel(canvas.get_pixel_mut(dest_x, dest_y), *pixel);
    }
}

fn blend_transformed_image(
    canvas: &mut RgbaImage,
    overlay: &RgbaImage,
    left: u32,
    top: u32,
    rotation_degrees: f32,
    flip_horizontal: bool,
    flip_vertical: bool,
) {
    if rotation_degrees.rem_euclid(360.0) == 0.0 && !flip_horizontal && !flip_vertical {
        blend_image(canvas, overlay, left, top);
        return;
    }
    let width = overlay.width() as f32;
    let height = overlay.height() as f32;
    let center = (left as f32 + width / 2.0, top as f32 + height / 2.0);
    let radians = rotation_degrees * PI / 180.0;
    let sin = radians.sin();
    let cos = radians.cos();
    let corners = [
        (-width / 2.0, -height / 2.0),
        (width / 2.0, -height / 2.0),
        (-width / 2.0, height / 2.0),
        (width / 2.0, height / 2.0),
    ];
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (x, y) in corners {
        let rx = x * cos - y * sin;
        let ry = x * sin + y * cos;
        min_x = min_x.min(center.0 + rx);
        min_y = min_y.min(center.1 + ry);
        max_x = max_x.max(center.0 + rx);
        max_y = max_y.max(center.1 + ry);
    }
    let start_x = min_x.floor().max(0.0) as u32;
    let start_y = min_y.floor().max(0.0) as u32;
    let end_x = max_x.ceil().min(canvas.width() as f32) as u32;
    let end_y = max_y.ceil().min(canvas.height() as f32) as u32;
    for dest_y in start_y..end_y {
        for dest_x in start_x..end_x {
            let mut local_x = dest_x as f32 + 0.5 - center.0;
            let mut local_y = dest_y as f32 + 0.5 - center.1;
            let src_x = local_x * cos + local_y * sin;
            let src_y = -local_x * sin + local_y * cos;
            local_x = if flip_horizontal { -src_x } else { src_x };
            local_y = if flip_vertical { -src_y } else { src_y };
            let sample_x = local_x + width / 2.0 - 0.5;
            let sample_y = local_y + height / 2.0 - 0.5;
            if sample_x < 0.0 || sample_y < 0.0 || sample_x >= width || sample_y >= height {
                continue;
            }
            let pixel = overlay.get_pixel(sample_x as u32, sample_y as u32);
            if pixel.0[3] == 0 {
                continue;
            }
            blend_pixel(canvas.get_pixel_mut(dest_x, dest_y), *pixel);
        }
    }
}

fn blend_pixel(target: &mut Rgba<u8>, source: Rgba<u8>) {
    let src_alpha = source.0[3] as f32 / 255.0;
    if src_alpha <= 0.0 {
        return;
    }
    let dst_alpha = target.0[3] as f32 / 255.0;
    let out_alpha = src_alpha + dst_alpha * (1.0 - src_alpha);
    for channel in 0..3 {
        let src = source.0[channel] as f32 / 255.0;
        let dst = target.0[channel] as f32 / 255.0;
        let out = (src * src_alpha + dst * dst_alpha * (1.0 - src_alpha)) / out_alpha.max(1e-6);
        target.0[channel] = (out * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    target.0[3] = (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
}

fn set_paint_color(paint: &mut Paint<'_>, color: Rgba<u8>) {
    paint.set_color_rgba8(color.0[0], color.0[1], color.0[2], color.0[3]);
}

fn parse_rgba(hex: &str, alpha: u8) -> Rgba<u8> {
    let value = hex.trim().trim_start_matches('#');
    if value.len() == 6
        && let Ok(rgb) = u32::from_str_radix(value, 16)
    {
        return Rgba([
            ((rgb >> 16) & 0xFF) as u8,
            ((rgb >> 8) & 0xFF) as u8,
            (rgb & 0xFF) as u8,
            alpha,
        ]);
    }
    Rgba([0, 0, 0, alpha])
}

fn shape_path(
    geometry: ShapeGeometry,
    width: u32,
    height: u32,
) -> Result<TinyPath, PresentationArtifactError> {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    match geometry {
        ShapeGeometry::Rectangle | ShapeGeometry::RoundedRectangle => {
            polygon_path(&[(0.0, 0.0), (width, 0.0), (width, height), (0.0, height)])
        }
        ShapeGeometry::Ellipse => ellipse_path(width as u32, height as u32),
        ShapeGeometry::Triangle => polygon_path(&[(width / 2.0, 0.0), (width, height), (0.0, height)]),
        ShapeGeometry::RightTriangle => polygon_path(&[(0.0, 0.0), (width, height), (0.0, height)]),
        ShapeGeometry::Diamond => polygon_path(&[
            (width / 2.0, 0.0),
            (width, height / 2.0),
            (width / 2.0, height),
            (0.0, height / 2.0),
        ]),
        ShapeGeometry::Pentagon => regular_polygon_path(5, width, height, -PI / 2.0),
        ShapeGeometry::Hexagon => regular_polygon_path(6, width, height, 0.0),
        ShapeGeometry::Octagon => regular_polygon_path(8, width, height, PI / 8.0),
        ShapeGeometry::Star4 => star_path(4, width, height),
        ShapeGeometry::Star5 => star_path(5, width, height),
        ShapeGeometry::Star6 => star_path(6, width, height),
        ShapeGeometry::Star8 => star_path(8, width, height),
        ShapeGeometry::RightArrow => polygon_path(&[
            (0.0, height * 0.25),
            (width * 0.55, height * 0.25),
            (width * 0.55, 0.0),
            (width, height * 0.5),
            (width * 0.55, height),
            (width * 0.55, height * 0.75),
            (0.0, height * 0.75),
        ]),
        ShapeGeometry::LeftArrow => polygon_path(&[
            (width, height * 0.25),
            (width * 0.45, height * 0.25),
            (width * 0.45, 0.0),
            (0.0, height * 0.5),
            (width * 0.45, height),
            (width * 0.45, height * 0.75),
            (width, height * 0.75),
        ]),
        ShapeGeometry::UpArrow => polygon_path(&[
            (width * 0.25, height),
            (width * 0.25, height * 0.45),
            (0.0, height * 0.45),
            (width * 0.5, 0.0),
            (width, height * 0.45),
            (width * 0.75, height * 0.45),
            (width * 0.75, height),
        ]),
        ShapeGeometry::DownArrow => polygon_path(&[
            (width * 0.25, 0.0),
            (width * 0.25, height * 0.55),
            (0.0, height * 0.55),
            (width * 0.5, height),
            (width, height * 0.55),
            (width * 0.75, height * 0.55),
            (width * 0.75, 0.0),
        ]),
        ShapeGeometry::LeftRightArrow => polygon_path(&[
            (0.0, height * 0.5),
            (width * 0.18, 0.0),
            (width * 0.18, height * 0.25),
            (width * 0.82, height * 0.25),
            (width * 0.82, 0.0),
            (width, height * 0.5),
            (width * 0.82, height),
            (width * 0.82, height * 0.75),
            (width * 0.18, height * 0.75),
            (width * 0.18, height),
        ]),
        ShapeGeometry::UpDownArrow => polygon_path(&[
            (width * 0.5, 0.0),
            (0.0, height * 0.18),
            (width * 0.25, height * 0.18),
            (width * 0.25, height * 0.82),
            (0.0, height * 0.82),
            (width * 0.5, height),
            (width, height * 0.82),
            (width * 0.75, height * 0.82),
            (width * 0.75, height * 0.18),
            (width, height * 0.18),
        ]),
        ShapeGeometry::Chevron => polygon_path(&[
            (0.0, 0.0),
            (width * 0.58, 0.0),
            (width, height * 0.5),
            (width * 0.58, height),
            (0.0, height),
            (width * 0.42, height * 0.5),
        ]),
        ShapeGeometry::Heart => heart_path(width, height),
        ShapeGeometry::Cloud => cloud_path(width, height),
        ShapeGeometry::Wave => wave_path(width, height),
        ShapeGeometry::FlowChartProcess
        | ShapeGeometry::FlowChartDecision
        | ShapeGeometry::FlowChartConnector
        | ShapeGeometry::Parallelogram
        | ShapeGeometry::Trapezoid => polygon_path(&[
            (width * 0.12, 0.0),
            (width, 0.0),
            (width * 0.88, height),
            (0.0, height),
        ]),
    }
}

fn polygon_path(points: &[(f32, f32)]) -> Result<TinyPath, PresentationArtifactError> {
    let mut builder = PathBuilder::new();
    let Some((first_x, first_y)) = points.first().copied() else {
        return Err(PresentationArtifactError::RenderFailed {
            action: "render_preview".to_string(),
            message: "polygon path had no points".to_string(),
        });
    };
    builder.move_to(first_x, first_y);
    for (x, y) in points.iter().copied().skip(1) {
        builder.line_to(x, y);
    }
    builder.close();
    builder.finish().ok_or_else(|| PresentationArtifactError::RenderFailed {
        action: "render_preview".to_string(),
        message: "failed to build polygon path".to_string(),
    })
}

fn polyline_path(points: &[(f32, f32)]) -> Result<TinyPath, PresentationArtifactError> {
    let mut builder = PathBuilder::new();
    let Some((first_x, first_y)) = points.first().copied() else {
        return Err(PresentationArtifactError::RenderFailed {
            action: "render_preview".to_string(),
            message: "polyline path had no points".to_string(),
        });
    };
    builder.move_to(first_x, first_y);
    for (x, y) in points.iter().copied().skip(1) {
        builder.line_to(x, y);
    }
    builder.finish().ok_or_else(|| PresentationArtifactError::RenderFailed {
        action: "render_preview".to_string(),
        message: "failed to build polyline path".to_string(),
    })
}

fn ellipse_path(width: u32, height: u32) -> Result<TinyPath, PresentationArtifactError> {
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let rx = width as f32 / 2.0;
    let ry = height as f32 / 2.0;
    let points = (0..32)
        .map(|index| {
            let angle = index as f32 / 32.0 * PI * 2.0;
            (cx + rx * angle.cos(), cy + ry * angle.sin())
        })
        .collect::<Vec<_>>();
    polygon_path(&points)
}

fn regular_polygon_path(
    sides: usize,
    width: f32,
    height: f32,
    rotation: f32,
) -> Result<TinyPath, PresentationArtifactError> {
    let cx = width / 2.0;
    let cy = height / 2.0;
    let radius = width.min(height) / 2.0;
    let points = (0..sides)
        .map(|index| {
            let angle = rotation + index as f32 * PI * 2.0 / sides as f32;
            (cx + radius * angle.cos(), cy + radius * angle.sin())
        })
        .collect::<Vec<_>>();
    polygon_path(&points)
}

fn star_path(points: usize, width: f32, height: f32) -> Result<TinyPath, PresentationArtifactError> {
    let cx = width / 2.0;
    let cy = height / 2.0;
    let outer = width.min(height) / 2.0;
    let inner = outer * 0.45;
    let mut vertices = Vec::with_capacity(points * 2);
    for index in 0..points * 2 {
        let angle = -PI / 2.0 + index as f32 * PI / points as f32;
        let radius = if index % 2 == 0 { outer } else { inner };
        vertices.push((cx + radius * angle.cos(), cy + radius * angle.sin()));
    }
    polygon_path(&vertices)
}

fn heart_path(width: f32, height: f32) -> Result<TinyPath, PresentationArtifactError> {
    let mut builder = PathBuilder::new();
    builder.move_to(width / 2.0, height);
    builder.cubic_to(width * 0.1, height * 0.7, -width * 0.05, height * 0.35, width * 0.25, height * 0.2);
    builder.cubic_to(width * 0.45, 0.0, width * 0.5, height * 0.15, width / 2.0, height * 0.25);
    builder.cubic_to(width * 0.5, height * 0.15, width * 0.55, 0.0, width * 0.75, height * 0.2);
    builder.cubic_to(width * 1.05, height * 0.35, width * 0.9, height * 0.7, width / 2.0, height);
    builder.close();
    builder.finish().ok_or_else(|| PresentationArtifactError::RenderFailed {
        action: "render_preview".to_string(),
        message: "failed to build heart path".to_string(),
    })
}

fn cloud_path(width: f32, height: f32) -> Result<TinyPath, PresentationArtifactError> {
    let mut builder = PathBuilder::new();
    builder.move_to(width * 0.2, height * 0.75);
    builder.cubic_to(0.0, height * 0.75, 0.0, height * 0.35, width * 0.22, height * 0.35);
    builder.cubic_to(width * 0.22, height * 0.1, width * 0.52, height * 0.05, width * 0.6, height * 0.3);
    builder.cubic_to(width * 0.85, height * 0.18, width, height * 0.45, width * 0.85, height * 0.68);
    builder.cubic_to(width * 0.96, height * 0.95, width * 0.64, height, width * 0.55, height * 0.82);
    builder.cubic_to(width * 0.42, height, width * 0.22, height * 0.96, width * 0.2, height * 0.75);
    builder.close();
    builder.finish().ok_or_else(|| PresentationArtifactError::RenderFailed {
        action: "render_preview".to_string(),
        message: "failed to build cloud path".to_string(),
    })
}

fn wave_path(width: f32, height: f32) -> Result<TinyPath, PresentationArtifactError> {
    let mut builder = PathBuilder::new();
    builder.move_to(0.0, height * 0.55);
    builder.cubic_to(width * 0.18, 0.0, width * 0.32, height, width * 0.5, height * 0.55);
    builder.cubic_to(width * 0.68, 0.1, width * 0.82, height, width, height * 0.45);
    builder.line_to(width, height);
    builder.line_to(0.0, height);
    builder.close();
    builder.finish().ok_or_else(|| PresentationArtifactError::RenderFailed {
        action: "render_preview".to_string(),
        message: "failed to build wave path".to_string(),
    })
}

fn connector_path(
    kind: ConnectorKind,
    start: (f32, f32),
    end: (f32, f32),
) -> Result<TinyPath, PresentationArtifactError> {
    match kind {
        ConnectorKind::Straight => polyline_path(&[start, end]),
        ConnectorKind::Elbow => polyline_path(&[start, (start.0, end.1), end]),
        ConnectorKind::Curved => {
            let mut builder = PathBuilder::new();
            builder.move_to(start.0, start.1);
            let mid_x = (start.0 + end.0) / 2.0;
            builder.cubic_to(mid_x, start.1, mid_x, end.1, end.0, end.1);
            builder.finish().ok_or_else(|| PresentationArtifactError::RenderFailed {
                action: "render_preview".to_string(),
                message: "failed to build connector path".to_string(),
            })
        }
    }
}

fn draw_arrow_head(
    pixmap: &mut Pixmap,
    tip: (f32, f32),
    tail: (f32, f32),
    kind: ConnectorArrowKind,
    size: f32,
    color: &str,
) -> Result<(), PresentationArtifactError> {
    if kind == ConnectorArrowKind::None || size <= 0.0 {
        return Ok(());
    }
    let dx = tip.0 - tail.0;
    let dy = tip.1 - tail.1;
    let length = (dx * dx + dy * dy).sqrt().max(1.0);
    let ux = dx / length;
    let uy = dy / length;
    let px = -uy;
    let py = ux;
    let points = match kind {
        ConnectorArrowKind::Triangle | ConnectorArrowKind::Stealth | ConnectorArrowKind::Open => vec![
            tip,
            (tip.0 - ux * size + px * size * 0.45, tip.1 - uy * size + py * size * 0.45),
            (tip.0 - ux * size - px * size * 0.45, tip.1 - uy * size - py * size * 0.45),
        ],
        ConnectorArrowKind::Diamond => vec![
            tip,
            (tip.0 - ux * size * 0.5 + px * size * 0.35, tip.1 - uy * size * 0.5 + py * size * 0.35),
            (tip.0 - ux * size, tip.1 - uy * size),
            (tip.0 - ux * size * 0.5 - px * size * 0.35, tip.1 - uy * size * 0.5 - py * size * 0.35),
        ],
        ConnectorArrowKind::Oval => {
            let path = ellipse_path(size.max(2.0) as u32, size.max(2.0) as u32)?;
            let marker = render_vector_image(size.max(2.0) as u32, size.max(2.0) as u32, |marker| {
                let mut paint = Paint::default();
                set_paint_color(&mut paint, parse_rgba(color, 255));
                marker.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                Ok(())
            })?;
            let overlay_left = tip.0.round().max(0.0) as u32;
            let overlay_top = tip.1.round().max(0.0) as u32;
            let mut image = RgbaImage::from_raw(pixmap.width(), pixmap.height(), pixmap.data().to_vec())
                .ok_or_else(|| PresentationArtifactError::RenderFailed {
                    action: "render_preview".to_string(),
                    message: "failed to build arrow image".to_string(),
                })?;
            blend_image(
                &mut image,
                &marker,
                overlay_left.saturating_sub(marker.width() / 2),
                overlay_top.saturating_sub(marker.height() / 2),
            );
            let data = image.into_raw();
            pixmap.data_mut().copy_from_slice(&data);
            return Ok(());
        }
        ConnectorArrowKind::None => return Ok(()),
    };
    let path = polygon_path(&points)?;
    let mut paint = Paint::default();
    set_paint_color(&mut paint, parse_rgba(color, 255));
    if kind == ConnectorArrowKind::Open {
        let stroke = Stroke {
            width: 1.5,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    } else {
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
    Ok(())
}

fn crop_dynamic_image(
    image: DynamicImage,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
) -> DynamicImage {
    let width = image.width();
    let height = image.height();
    let crop_left = (width as f64 * left).round().clamp(0.0, width as f64) as u32;
    let crop_top = (height as f64 * top).round().clamp(0.0, height as f64) as u32;
    let crop_right = (width as f64 * right).round().clamp(0.0, width as f64) as u32;
    let crop_bottom = (height as f64 * bottom).round().clamp(0.0, height as f64) as u32;
    let cropped_width = width.saturating_sub(crop_left.saturating_add(crop_right)).max(1);
    let cropped_height = height.saturating_sub(crop_top.saturating_add(crop_bottom)).max(1);
    image.crop_imm(crop_left, crop_top, cropped_width, cropped_height)
}

fn build_table_merge_lookup(merges: &[TableMergeRegion]) -> Vec<TableMergeRegion> {
    merges.to_vec()
}

fn merged_span(
    merges: &[TableMergeRegion],
    row: usize,
    column: usize,
) -> Option<(usize, usize)> {
    merges.iter().find_map(|merge| {
        (merge.start_row == row && merge.start_column == column).then_some((
            merge.end_row.saturating_sub(merge.start_row) + 1,
            merge.end_column.saturating_sub(merge.start_column) + 1,
        ))
    })
}

fn is_merged_away(merges: &[TableMergeRegion], row: usize, column: usize) -> bool {
    merges.iter().any(|merge| {
        row >= merge.start_row
            && row <= merge.end_row
            && column >= merge.start_column
            && column <= merge.end_column
            && !(merge.start_row == row && merge.start_column == column)
    })
}

fn table_cell_fill(table: &TableElement, row: usize, column: usize) -> Rgba<u8> {
    if row == 0 && table.style_options.header_row {
        return parse_rgba("EAF0F8", 255);
    }
    if table.style_options.banded_rows && row % 2 == 1 {
        return parse_rgba("F7F9FB", 255);
    }
    if table.style_options.banded_columns && column % 2 == 1 {
        return parse_rgba("F7F9FB", 255);
    }
    Rgba([255, 255, 255, 255])
}

fn draw_rect_border(
    image: &mut RgbaImage,
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    color: Rgba<u8>,
    thickness: u32,
) {
    fill_rect(image, left, top, width, thickness, color);
    fill_rect(
        image,
        left,
        top.saturating_add(height.saturating_sub(thickness)),
        width,
        thickness,
        color,
    );
    fill_rect(image, left, top, thickness, height, color);
    fill_rect(
        image,
        left.saturating_add(width.saturating_sub(thickness)),
        top,
        thickness,
        height,
        color,
    );
}

fn draw_horizontal_line(image: &mut RgbaImage, y: u32, color: Rgba<u8>, thickness: u32) {
    fill_rect(image, 0, y.saturating_sub(thickness / 2), image.width(), thickness.max(1), color);
}

fn draw_vertical_line(image: &mut RgbaImage, x: u32, color: Rgba<u8>, thickness: u32) {
    fill_rect(image, x.saturating_sub(thickness / 2), 0, thickness.max(1), image.height(), color);
}

fn draw_chart_axes(image: &mut RgbaImage, left: u32, top: u32, width: u32, height: u32) {
    let axis_color = parse_rgba(DEFAULT_TABLE_GRID_HEX, 255);
    fill_rect(image, left, top, 1, height, axis_color);
    fill_rect(image, left, top + height, width, 1, axis_color);
}

fn render_category_labels(
    image: &mut RgbaImage,
    categories: &[String],
    left: u32,
    top: u32,
    width: u32,
    scale: f32,
) {
    if categories.is_empty() {
        return;
    }
    let step = width as f32 / categories.len().max(1) as f32;
    for (index, category) in categories.iter().enumerate() {
        let x = left + (step * index as f32) as u32;
        draw_text_in_bounds(
            image,
            TextBounds {
                left: x,
                top,
                width: step.round().max(1.0) as u32,
                height: scaled_dimension(20, scale),
            },
            category,
            &TextStyle {
                font_size: Some(10),
                color: Some(DEFAULT_TEXT_HEX.to_string()),
                alignment: Some(TextAlignment::Center),
                ..TextStyle::default()
            },
            &RichTextState::default(),
            None,
            scale,
        );
    }
}

fn chart_series_color(series: &ChartSeriesSpec, index: usize) -> Rgba<u8> {
    series
        .fill
        .as_deref()
        .map(|fill| parse_rgba(fill, 255))
        .unwrap_or_else(|| chart_palette_color(index))
}

fn chart_palette_color(index: usize) -> Rgba<u8> {
    let palette = ["5B8FF9", "5AD8A6", "5D7092", "F6BD16", "E8684A", "6DC8EC", "9270CA"];
    parse_rgba(palette[index % palette.len()], 255)
}

fn sector_path(
    center: (f32, f32),
    radius: f32,
    start: f32,
    end: f32,
    doughnut: bool,
) -> Result<TinyPath, PresentationArtifactError> {
    let steps = (((end - start).abs() / (PI / 18.0)).ceil() as usize).max(2);
    let mut points = Vec::new();
    if !doughnut {
        points.push(center);
    }
    for index in 0..=steps {
        let angle = start + (end - start) * index as f32 / steps as f32;
        points.push((center.0 + radius * angle.cos(), center.1 + radius * angle.sin()));
    }
    if doughnut {
        let inner = radius * 0.55;
        for index in (0..=steps).rev() {
            let angle = start + (end - start) * index as f32 / steps as f32;
            points.push((center.0 + inner * angle.cos(), center.1 + inner * angle.sin()));
        }
    }
    polygon_path(&points)
}

fn resolve_render_slide_index(
    document: &PresentationDocument,
    slide_index: Option<u32>,
    action: &str,
) -> Result<usize, PresentationArtifactError> {
    if let Some(slide_index) = slide_index {
        let slide_index = usize::try_from(slide_index).map_err(|_| {
            PresentationArtifactError::InvalidArgs {
                action: action.to_string(),
                message: "`slide_index` does not fit in usize".to_string(),
            }
        })?;
        if slide_index >= document.slides.len() {
            return Err(index_out_of_range(action, slide_index, document.slides.len()));
        }
        return Ok(slide_index);
    }
    if let Some(active_slide_index) = document.active_slide_index {
        return Ok(active_slide_index);
    }
    if document.slides.is_empty() {
        return Err(PresentationArtifactError::InvalidArgs {
            action: action.to_string(),
            message: "presentation has no slides".to_string(),
        });
    }
    Ok(0)
}
