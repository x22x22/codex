use std::collections::BTreeMap;

use pretty_assertions::assert_eq;

use crate::CellAddress;
use crate::CellRange;
use crate::SpreadsheetArtifact;
use crate::SpreadsheetArtifactManager;
use crate::SpreadsheetArtifactRequest;
use crate::SpreadsheetCell;
use crate::SpreadsheetCellFormat;
use crate::SpreadsheetCellFormatSummary;
use crate::SpreadsheetCellValue;
use crate::SpreadsheetFileType;
use crate::SpreadsheetFill;
use crate::SpreadsheetFontFace;
use crate::SpreadsheetNumberFormat;
use crate::SpreadsheetRenderOptions;
use crate::SpreadsheetSheet;
use crate::SpreadsheetSheetReference;
use crate::SpreadsheetTextStyle;

#[test]
fn manager_can_create_edit_recalculate_and_export() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let mut manager = SpreadsheetArtifactManager::default();

    let created = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: None,
            action: "create".to_string(),
            args: serde_json::json!({ "name": "Budget" }),
        },
        temp_dir.path(),
    )?;
    let artifact_id = created.artifact_id;

    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_sheet".to_string(),
            args: serde_json::json!({ "name": "Sheet1" }),
        },
        temp_dir.path(),
    )?;

    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "set_range_values".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "range": "A1:B2",
                "values": [[1, 2], [3, 4]]
            }),
        },
        temp_dir.path(),
    )?;

    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "set_cell_formula".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "address": "C1",
                "formula": "=SUM(A1:B2)",
                "recalculate": true
            }),
        },
        temp_dir.path(),
    )?;

    let cell = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "get_cell".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "address": "C1"
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(
        cell.cell.and_then(|entry| entry.value),
        Some(SpreadsheetCellValue::Integer(10))
    );

    let export_path = temp_dir.path().join("budget.xlsx");
    let export = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id),
            action: "export_xlsx".to_string(),
            args: serde_json::json!({ "path": export_path }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(export.exported_paths.len(), 1);
    assert!(export.exported_paths[0].exists());
    Ok(())
}

#[test]
fn spreadsheet_serialization_roundtrip_preserves_cells() -> Result<(), Box<dyn std::error::Error>> {
    let mut artifact = SpreadsheetArtifact::new(Some("Roundtrip".to_string()));
    let sheet = artifact.create_sheet("Sheet1".to_string())?;
    sheet.set_value(
        crate::CellAddress::parse("A1")?,
        Some(SpreadsheetCellValue::String("hello".to_string())),
    )?;
    sheet.set_formula(crate::CellAddress::parse("B1")?, Some("=A1".to_string()))?;
    artifact.recalculate();

    let json = artifact.to_json()?;
    let restored = SpreadsheetArtifact::from_json(json, None)?;
    let restored_sheet = restored.get_sheet(Some("Sheet1"), None).expect("sheet");
    let cell = restored_sheet.get_cell_view(crate::CellAddress::parse("A1")?);
    assert_eq!(
        cell.value,
        Some(SpreadsheetCellValue::String("hello".to_string()))
    );
    Ok(())
}

#[test]
fn xlsx_roundtrip_preserves_merged_ranges_and_style_indices()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("styled.xlsx");

    let mut artifact = SpreadsheetArtifact::new(Some("Styled".to_string()));
    let sheet = artifact.create_sheet("Sheet1".to_string())?;
    sheet.set_value(
        crate::CellAddress::parse("A1")?,
        Some(SpreadsheetCellValue::Integer(42)),
    )?;
    sheet.set_style_index(&crate::CellRange::parse("A1:B1")?, 3)?;
    sheet.merge_cells(&crate::CellRange::parse("A1:B1")?, true)?;
    artifact.export(&path)?;

    let restored = SpreadsheetArtifact::from_source_file(&path, None)?;
    let restored_sheet = restored.get_sheet(Some("Sheet1"), None).expect("sheet");
    assert_eq!(restored_sheet.merged_ranges.len(), 1);
    assert_eq!(
        restored_sheet
            .get_cell_view(crate::CellAddress::parse("A1")?)
            .style_index,
        3
    );
    Ok(())
}

#[test]
fn path_accesses_cover_import_and_export() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = tempfile::tempdir()?;
    let request = crate::SpreadsheetArtifactRequest {
        artifact_id: Some("spreadsheet_1".to_string()),
        action: "export_xlsx".to_string(),
        args: serde_json::json!({ "path": "out/report.xlsx" }),
    };
    let accesses = request.required_path_accesses(cwd.path())?;
    assert_eq!(accesses.len(), 1);
    assert!(accesses[0].path.ends_with("out/report.xlsx"));
    Ok(())
}

#[test]
fn render_options_write_deterministic_html_previews() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let mut artifact = SpreadsheetArtifact::new(Some("Preview".to_string()));
    artifact.create_sheet("Sheet 1".to_string())?;
    {
        let sheet = artifact
            .get_sheet_mut(Some("Sheet 1"), None)
            .expect("sheet");
        sheet.set_value(
            CellAddress::parse("A1")?,
            Some(SpreadsheetCellValue::String("Name".to_string())),
        )?;
        sheet.set_value(
            CellAddress::parse("B1")?,
            Some(SpreadsheetCellValue::String("Value".to_string())),
        )?;
        sheet.set_value(
            CellAddress::parse("A2")?,
            Some(SpreadsheetCellValue::String("Alpha".to_string())),
        )?;
        sheet.set_value(
            CellAddress::parse("B2")?,
            Some(SpreadsheetCellValue::Integer(42)),
        )?;
    }

    let rendered = artifact.render_range_preview(
        temp_dir.path(),
        artifact.get_sheet(Some("Sheet 1"), None).expect("sheet"),
        &CellRange::parse("A1:B2")?,
        &SpreadsheetRenderOptions {
            output_path: Some(temp_dir.path().join("range-preview.html")),
            width: Some(320),
            height: Some(200),
            include_headers: true,
            scale: 1.25,
            performance_mode: true,
            ..Default::default()
        },
    )?;
    assert!(rendered.path.exists());
    assert_eq!(std::fs::read_to_string(&rendered.path)?, rendered.html);
    assert!(rendered.html.contains("<!doctype html>"));
    assert!(rendered.html.contains("data-performance-mode=\"true\""));
    assert!(rendered.html.contains(
        "style=\"--scale: 1.25; --headers: 1; width: 320px; height: 200px; overflow: auto\""
    ));
    assert!(rendered.html.contains("<th>A</th>"));
    assert!(rendered.html.contains("data-address=\"B2\""));
    assert!(rendered.html.contains(">42</td>"));

    let workbook = artifact.render_workbook_previews(
        temp_dir.path(),
        &SpreadsheetRenderOptions {
            output_path: Some(temp_dir.path().join("workbook")),
            include_headers: false,
            ..Default::default()
        },
    )?;
    assert_eq!(workbook.len(), 1);
    assert!(workbook[0].path.ends_with("Sheet_1.html"));
    assert!(!workbook[0].html.contains("<th>A</th>"));
    Ok(())
}

#[test]
fn sheet_refs_support_handle_and_field_apis() -> Result<(), Box<dyn std::error::Error>> {
    let mut artifact = SpreadsheetArtifact::new(Some("Handles".to_string()));
    let (range_ref, cell_ref) = {
        let sheet = artifact.create_sheet("Sheet1".to_string())?;
        let range_ref = sheet.range_ref("A1:B2")?;
        range_ref.set_value(sheet, Some(SpreadsheetCellValue::Integer(7)))?;
        let cell_ref = sheet.cell_ref("B2")?;
        cell_ref.set_formula(sheet, Some("=SUM(A1:B2)".to_string()))?;
        (range_ref, cell_ref)
    };
    artifact.recalculate();
    let sheet = artifact.get_sheet(Some("Sheet1"), None).expect("sheet");

    let values = range_ref.get_values(sheet)?;
    assert_eq!(values[0][0], Some(SpreadsheetCellValue::Integer(7)));
    assert_eq!(
        cell_ref.get(sheet)?.value,
        Some(SpreadsheetCellValue::Integer(28))
    );
    assert_eq!(
        sheet.get_cell_field_by_indices(2, 2, "formula")?,
        Some(serde_json::Value::String("=SUM(A1:B2)".to_string()))
    );
    assert_eq!(
        sheet.minimum_range_ref().map(|entry| entry.address),
        Some("A1:B2".to_string())
    );
    assert!(matches!(
        sheet.to_dict()?,
        serde_json::Value::Object(_) | serde_json::Value::Array(_)
    ));
    Ok(())
}

#[test]
fn manager_supports_single_value_formula_and_cite_cell_actions()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let mut manager = SpreadsheetArtifactManager::default();
    let created = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: None,
            action: "create".to_string(),
            args: serde_json::json!({ "name": "Actions" }),
        },
        temp_dir.path(),
    )?;
    let artifact_id = created.artifact_id;

    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_sheet".to_string(),
            args: serde_json::json!({ "name": "Sheet1" }),
        },
        temp_dir.path(),
    )?;

    let uniform = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "set_range_value".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "range": "A1:B2",
                "value": 5
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(
        uniform
            .range_ref
            .as_ref()
            .map(|entry| entry.address.clone()),
        Some("A1:B2".to_string())
    );

    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "set_range_formula".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "range": "C1:C2",
                "formula": "=SUM(A1:B2)",
                "recalculate": true
            }),
        },
        temp_dir.path(),
    )?;

    let cited = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "cite_cell".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "address": "C1",
                "tether_id": "source-1",
                "start_line": 3,
                "end_line": 8
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(
        cited.cell.as_ref().map(|entry| entry.citations.len()),
        Some(1)
    );

    let by_indices = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "get_cell_by_indices".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "column_index": 3,
                "row_index": 1
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(
        by_indices
            .cell
            .as_ref()
            .and_then(|entry| entry.value.clone()),
        Some(SpreadsheetCellValue::Integer(20))
    );

    let field = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id),
            action: "get_cell_field".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "address": "C1",
                "field": "formula"
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(
        field.cell_field,
        Some(serde_json::Value::String("=SUM(A1:B2)".to_string()))
    );
    Ok(())
}

#[test]
fn artifact_file_type_helpers_and_source_files_work() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let mut artifact = SpreadsheetArtifact::new(Some("Files".to_string()));
    artifact.artifact_id = "spreadsheet_fixed".to_string();
    artifact.create_sheet("Sheet1".to_string())?.set_value(
        CellAddress::parse("A1")?,
        Some(SpreadsheetCellValue::String("hello".to_string())),
    )?;

    assert_eq!(
        SpreadsheetArtifact::allowed_file_extensions(),
        &["xlsx", "json", "bin"]
    );
    assert_eq!(
        SpreadsheetArtifact::allowed_file_mime_types(),
        &[
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "application/json",
            "application/octet-stream",
        ]
    );
    assert_eq!(
        SpreadsheetArtifact::allowed_file_types().to_vec(),
        vec![
            SpreadsheetFileType::Xlsx,
            SpreadsheetFileType::Json,
            SpreadsheetFileType::Binary,
        ]
    );
    assert_eq!(
        artifact.get_output_file_name(Some("preview"), SpreadsheetFileType::Json),
        "spreadsheet_fixed_preview.json".to_string()
    );

    let json_path = temp_dir
        .path()
        .join(artifact.get_output_file_name(None, SpreadsheetFileType::Json));
    artifact.save(&json_path, Some("json"))?;
    let restored_json = SpreadsheetArtifact::load(&json_path, None)?;
    assert_eq!(restored_json.to_dict()?, artifact.to_dict()?);

    let bytes_path = temp_dir
        .path()
        .join(artifact.get_output_file_name(Some("bytes"), SpreadsheetFileType::Binary));
    artifact.save(&bytes_path, Some("bin"))?;
    let restored_bytes = SpreadsheetArtifact::read(&bytes_path, None)?;
    assert_eq!(restored_bytes.to_dict()?, artifact.to_dict()?);
    Ok(())
}

#[test]
fn sheet_cleanup_and_row_sizing_helpers_work() -> Result<(), Box<dyn std::error::Error>> {
    let mut sheet = SpreadsheetSheet::new("Sheet1".to_string());
    sheet.default_row_height = Some(15.0);
    sheet.set_column_widths_bulk(&BTreeMap::from([
        ("A".to_string(), 12.0),
        ("C:D".to_string(), 20.0),
    ]))?;
    sheet.set_row_height(2, Some(18.0))?;
    sheet.set_row_heights(3, 4, Some(22.0))?;
    sheet.set_row_heights_bulk(&BTreeMap::from([(4, None), (5, Some(30.0))]))?;

    assert_eq!(sheet.get_column_width("A")?, Some(12.0));
    assert_eq!(sheet.get_column_width("B")?, None);
    assert_eq!(sheet.get_column_width("D")?, Some(20.0));
    assert_eq!(sheet.get_row_height(2), Some(18.0));
    assert_eq!(sheet.get_row_height(3), Some(22.0));
    assert_eq!(sheet.get_row_height(4), Some(15.0));
    assert_eq!(sheet.get_row_height(5), Some(30.0));

    sheet.cells.insert(
        CellAddress::parse("A1")?,
        SpreadsheetCell {
            value: None,
            formula: None,
            style_index: 0,
            citations: Vec::new(),
        },
    );
    let merged = CellRange::parse("B2:C3")?;
    sheet.merged_ranges = vec![merged.clone(), merged.clone()];
    sheet.cleanup_and_validate_sheet()?;

    assert_eq!(sheet.cells, BTreeMap::new());
    assert_eq!(sheet.merged_ranges, vec![merged]);
    Ok(())
}

#[test]
fn xlsx_roundtrip_preserves_row_and_column_sizes() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("sizing.xlsx");

    let mut artifact = SpreadsheetArtifact::new(Some("Sizing".to_string()));
    let (expected_column_widths, expected_row_heights, expected_show_grid_lines) = {
        let sheet = artifact.create_sheet("Sheet1".to_string())?;
        sheet.show_grid_lines = false;
        sheet.set_value(
            CellAddress::parse("A1")?,
            Some(SpreadsheetCellValue::Integer(42)),
        )?;
        sheet.set_column_widths_bulk(&BTreeMap::from([
            ("A:B".to_string(), 12.5),
            ("D".to_string(), 18.0),
        ]))?;
        sheet.set_row_heights_bulk(&BTreeMap::from([(2, Some(24.0)), (6, Some(19.5))]))?;
        (
            sheet.column_widths.clone(),
            sheet.row_heights.clone(),
            sheet.show_grid_lines,
        )
    };
    artifact.export(&path)?;

    let restored = SpreadsheetArtifact::from_source_file(&path, None)?;
    let restored_sheet = restored.get_sheet(Some("Sheet1"), None).expect("sheet");
    assert_eq!(restored_sheet.column_widths, expected_column_widths);
    assert_eq!(restored_sheet.row_heights, expected_row_heights);
    assert_eq!(restored_sheet.show_grid_lines, expected_show_grid_lines);
    Ok(())
}

#[test]
fn xlsx_roundtrip_preserves_style_registry_and_blank_styled_cells()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("styled-roundtrip.xlsx");

    let mut artifact = SpreadsheetArtifact::new(Some("Styled Roundtrip".to_string()));
    let style_index = {
        let text_style_id = artifact.create_text_style(
            SpreadsheetTextStyle {
                bold: Some(true),
                italic: Some(true),
                underline: Some(true),
                font_size: Some(14.0),
                font_color: Some("#112233".to_string()),
                font_family: Some("Aptos".to_string()),
                typeface: Some("Aptos".to_string()),
                ..Default::default()
            },
            None,
            false,
        )?;
        let fill_id = artifact.create_fill(
            SpreadsheetFill {
                solid_fill_color: Some("#FFEEAA".to_string()),
                pattern_type: Some("solid".to_string()),
                pattern_foreground_color: Some("#FFEEAA".to_string()),
                pattern_background_color: Some("#221100".to_string()),
                ..Default::default()
            },
            None,
            false,
        )?;
        let border_id = artifact.create_border(
            crate::SpreadsheetBorder {
                top: Some(crate::SpreadsheetBorderLine {
                    style: Some("thin".to_string()),
                    color: Some("#111111".to_string()),
                }),
                right: Some(crate::SpreadsheetBorderLine {
                    style: Some("dashed".to_string()),
                    color: Some("#222222".to_string()),
                }),
                bottom: Some(crate::SpreadsheetBorderLine {
                    style: Some("double".to_string()),
                    color: Some("#333333".to_string()),
                }),
                left: Some(crate::SpreadsheetBorderLine {
                    style: Some("hair".to_string()),
                    color: Some("#444444".to_string()),
                }),
            },
            None,
            false,
        )?;
        let number_format_id = artifact.create_number_format(
            SpreadsheetNumberFormat {
                format_id: Some(4),
                format_code: Some("#,##0.00".to_string()),
            },
            None,
            false,
        )?;
        artifact.create_cell_format(
            SpreadsheetCellFormat {
                text_style_id: Some(text_style_id),
                fill_id: Some(fill_id),
                border_id: Some(border_id),
                alignment: Some(crate::SpreadsheetAlignment {
                    horizontal: Some("center".to_string()),
                    vertical: Some("bottom".to_string()),
                }),
                number_format_id: Some(number_format_id),
                wrap_text: Some(true),
                base_cell_style_format_id: None,
            },
            None,
            false,
        )?
    };

    let sheet = artifact.create_sheet("Sheet1".to_string())?;
    sheet.set_value(
        CellAddress::parse("A1")?,
        Some(SpreadsheetCellValue::Float(42.5)),
    )?;
    sheet.set_style_index(&CellRange::parse("A1")?, style_index)?;
    sheet.set_style_index(&CellRange::parse("B2")?, style_index)?;
    artifact.export(&path)?;

    let restored = SpreadsheetArtifact::from_source_file(&path, None)?;
    let restored_sheet = restored.get_sheet(Some("Sheet1"), None).expect("sheet");

    assert_eq!(
        restored_sheet
            .get_raw_cell(CellAddress::parse("B2")?)
            .map(|cell| cell.style_index),
        Some(style_index)
    );
    assert_eq!(
        restored.cell_format_summary(style_index),
        Some(SpreadsheetCellFormatSummary {
            style_index,
            text_style: Some(SpreadsheetTextStyle {
                bold: Some(true),
                italic: Some(true),
                underline: Some(true),
                font_size: Some(14.0),
                font_color: Some("#112233".to_string()),
                text_alignment: None,
                anchor: None,
                vertical_text_orientation: None,
                text_rotation: None,
                paragraph_spacing: None,
                bottom_inset: None,
                left_inset: None,
                right_inset: None,
                top_inset: None,
                font_family: Some("Aptos".to_string()),
                font_scheme: None,
                typeface: Some("Aptos".to_string()),
                font_face: None,
            }),
            fill: Some(SpreadsheetFill {
                solid_fill_color: Some("#FFEEAA".to_string()),
                pattern_type: Some("solid".to_string()),
                pattern_foreground_color: Some("#FFEEAA".to_string()),
                pattern_background_color: Some("#221100".to_string()),
                color_transforms: Vec::new(),
                gradient_fill_type: None,
                gradient_stops: Vec::new(),
                gradient_kind: None,
                angle: None,
                scaled: None,
                path_type: None,
                fill_rectangle: None,
                image_reference: None,
            }),
            border: Some(crate::SpreadsheetBorder {
                top: Some(crate::SpreadsheetBorderLine {
                    style: Some("thin".to_string()),
                    color: Some("#111111".to_string()),
                }),
                right: Some(crate::SpreadsheetBorderLine {
                    style: Some("dashed".to_string()),
                    color: Some("#222222".to_string()),
                }),
                bottom: Some(crate::SpreadsheetBorderLine {
                    style: Some("double".to_string()),
                    color: Some("#333333".to_string()),
                }),
                left: Some(crate::SpreadsheetBorderLine {
                    style: Some("hair".to_string()),
                    color: Some("#444444".to_string()),
                }),
            }),
            alignment: Some(crate::SpreadsheetAlignment {
                horizontal: Some("center".to_string()),
                vertical: Some("bottom".to_string()),
            }),
            number_format: Some(SpreadsheetNumberFormat {
                format_id: Some(4),
                format_code: Some("#,##0.00".to_string()),
            }),
            wrap_text: Some(true),
        })
    );
    Ok(())
}

#[test]
fn xlsx_roundtrip_preserves_custom_sheet_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("metadata-roundtrip.xlsx");

    let mut artifact = SpreadsheetArtifact::new(Some("Metadata".to_string()));
    {
        let sheet = artifact.create_sheet("Sheet1".to_string())?;
        sheet.set_value(
            CellAddress::parse("A1")?,
            Some(SpreadsheetCellValue::String("Quarter".to_string())),
        )?;
        sheet.set_value(
            CellAddress::parse("B1")?,
            Some(SpreadsheetCellValue::String("Revenue".to_string())),
        )?;
        sheet.set_value(
            CellAddress::parse("C1")?,
            Some(SpreadsheetCellValue::String("Forecast".to_string())),
        )?;
        sheet.set_value(
            CellAddress::parse("A2")?,
            Some(SpreadsheetCellValue::String("Q1".to_string())),
        )?;
        sheet.set_value(
            CellAddress::parse("B2")?,
            Some(SpreadsheetCellValue::Integer(120)),
        )?;
        sheet.set_value(
            CellAddress::parse("C2")?,
            Some(SpreadsheetCellValue::Integer(118)),
        )?;
        sheet.create_table(
            "test_create_table",
            &CellRange::parse("A1:C2")?,
            crate::SpreadsheetCreateTableOptions {
                name: Some("Metrics".to_string()),
                display_name: Some("Metrics".to_string()),
                header_row_count: 1,
                totals_row_count: 0,
                style_name: Some("TableStyleMedium2".to_string()),
                show_row_stripes: true,
                ..Default::default()
            },
        )?;
        sheet.create_chart(
            "test_create_chart",
            crate::SpreadsheetChartType::Line,
            Some("Sheet1".to_string()),
            &CellRange::parse("A1:C2")?,
            crate::SpreadsheetChartCreateOptions {
                title: Some("Revenue vs Forecast".to_string()),
                legend_position: Some(crate::SpreadsheetChartLegendPosition::Right),
                ..Default::default()
            },
        )?;
        sheet.pivot_tables.push(crate::SpreadsheetPivotTable {
            name: "Pivot1".to_string(),
            cache_id: 7,
            address: Some("E1:G5".to_string()),
            row_fields: vec![crate::SpreadsheetPivotFieldReference {
                field_index: 0,
                field_name: Some("Quarter".to_string()),
            }],
            column_fields: Vec::new(),
            page_fields: Vec::new(),
            data_fields: vec![crate::SpreadsheetPivotDataField {
                field_index: 1,
                field_name: Some("Revenue".to_string()),
                name: Some("Sum of Revenue".to_string()),
                subtotal: Some("sum".to_string()),
            }],
            filters: Vec::new(),
            pivot_fields: vec![crate::SpreadsheetPivotField {
                index: 0,
                name: Some("Quarter".to_string()),
                axis: Some("axisRow".to_string()),
                items: vec![crate::SpreadsheetPivotFieldItem {
                    item_type: Some("default".to_string()),
                    index: Some(0),
                    hidden: false,
                }],
            }],
            style_name: Some("PivotStyleLight16".to_string()),
            part_path: Some("xl/pivotTables/pivotTable1.xml".to_string()),
        });
    }
    artifact.add_conditional_format(
        "test_add_conditional_format",
        "Sheet1",
        crate::SpreadsheetConditionalFormat {
            id: 0,
            range: "B2:C2".to_string(),
            rule_type: crate::SpreadsheetConditionalFormatType::ColorScale,
            operator: None,
            formulas: Vec::new(),
            text: None,
            dxf_id: None,
            stop_if_true: false,
            priority: 0,
            rank: None,
            percent: None,
            time_period: None,
            above_average: None,
            equal_average: None,
            color_scale: Some(crate::SpreadsheetColorScale {
                min_type: Some("min".to_string()),
                mid_type: Some("percentile".to_string()),
                max_type: Some("max".to_string()),
                min_value: None,
                mid_value: Some("50".to_string()),
                max_value: None,
                min_color: "#FFF2CC".to_string(),
                mid_color: Some("#FFD966".to_string()),
                max_color: "#BF9000".to_string(),
            }),
            data_bar: None,
            icon_set: None,
        },
    )?;

    artifact.export(&path)?;

    let restored = SpreadsheetArtifact::from_source_file(&path, None)?;
    let restored_sheet = restored.get_sheet(Some("Sheet1"), None).expect("sheet");
    assert_eq!(restored_sheet.tables.len(), 1);
    assert_eq!(restored_sheet.tables[0].name, "Metrics".to_string());
    assert_eq!(restored_sheet.charts.len(), 1);
    assert_eq!(
        restored_sheet.charts[0].title,
        Some("Revenue vs Forecast".to_string())
    );
    assert_eq!(restored_sheet.conditional_formats.len(), 1);
    assert_eq!(
        restored_sheet.conditional_formats[0].rule_type,
        crate::SpreadsheetConditionalFormatType::ColorScale
    );
    assert_eq!(restored_sheet.pivot_tables.len(), 1);
    assert_eq!(restored_sheet.pivot_tables[0].cache_id, 7);
    Ok(())
}

#[test]
fn xlsx_roundtrip_preserves_conditional_format_differential_styles()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("conditional-dxf-roundtrip.xlsx");

    let mut artifact = SpreadsheetArtifact::new(Some("Conditional DXF".to_string()));
    let fill_id = artifact.create_fill(
        SpreadsheetFill {
            solid_fill_color: Some("#FFCCCC".to_string()),
            pattern_type: Some("solid".to_string()),
            pattern_foreground_color: Some("#FFCCCC".to_string()),
            ..Default::default()
        },
        None,
        false,
    )?;
    let dxf_id = artifact.create_differential_format(crate::SpreadsheetDifferentialFormat {
        text_style_id: None,
        fill_id: Some(fill_id),
        border_id: None,
        alignment: None,
        number_format_id: None,
        wrap_text: None,
    })?;
    let sheet = artifact.create_sheet("Sheet1".to_string())?;
    sheet.set_value(
        CellAddress::parse("A1")?,
        Some(SpreadsheetCellValue::Integer(12)),
    )?;
    artifact.add_conditional_format(
        "test_add_conditional_format",
        "Sheet1",
        crate::SpreadsheetConditionalFormat {
            id: 0,
            range: "A1:A1".to_string(),
            rule_type: crate::SpreadsheetConditionalFormatType::Expression,
            operator: None,
            formulas: vec!["A1>10".to_string()],
            text: None,
            dxf_id: Some(dxf_id),
            stop_if_true: true,
            priority: 0,
            rank: None,
            percent: None,
            time_period: None,
            above_average: None,
            equal_average: None,
            color_scale: None,
            data_bar: None,
            icon_set: None,
        },
    )?;

    artifact.export(&path)?;

    let restored = SpreadsheetArtifact::from_source_file(&path, None)?;
    let restored_sheet = restored.get_sheet(Some("Sheet1"), None).expect("sheet");
    assert_eq!(restored_sheet.conditional_formats.len(), 1);
    assert_eq!(
        restored_sheet.conditional_formats[0],
        crate::SpreadsheetConditionalFormat {
            id: 1,
            range: "A1:A1".to_string(),
            rule_type: crate::SpreadsheetConditionalFormatType::Expression,
            operator: None,
            formulas: vec!["A1>10".to_string()],
            text: None,
            dxf_id: Some(dxf_id),
            stop_if_true: true,
            priority: 1,
            rank: None,
            percent: None,
            time_period: None,
            above_average: None,
            equal_average: None,
            color_scale: None,
            data_bar: None,
            icon_set: None,
        }
    );
    assert_eq!(
        restored.get_differential_format(dxf_id),
        Some(&crate::SpreadsheetDifferentialFormat {
            text_style_id: None,
            fill_id: Some(fill_id),
            border_id: None,
            alignment: None,
            number_format_id: None,
            wrap_text: None,
        })
    );
    Ok(())
}

#[test]
fn native_xlsx_table_import_reconstructs_table_metadata() -> Result<(), Box<dyn std::error::Error>>
{
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("native-table.xlsx");

    let mut artifact = SpreadsheetArtifact::new(Some("Native Table".to_string()));
    let sheet = artifact.create_sheet("Sheet1".to_string())?;
    sheet.set_value(
        CellAddress::parse("A1")?,
        Some(SpreadsheetCellValue::String("Quarter".to_string())),
    )?;
    sheet.set_value(
        CellAddress::parse("B1")?,
        Some(SpreadsheetCellValue::String("Revenue".to_string())),
    )?;
    sheet.set_value(
        CellAddress::parse("A2")?,
        Some(SpreadsheetCellValue::String("Q1".to_string())),
    )?;
    sheet.set_value(
        CellAddress::parse("B2")?,
        Some(SpreadsheetCellValue::Integer(120)),
    )?;
    artifact.export(&path)?;

    let sheet_xml = read_zip_entry_for_test(&path, "xl/worksheets/sheet1.xml")?;
    rewrite_xlsx_entries(
        &path,
        &BTreeMap::from([
            (
                "[Content_Types].xml".to_string(),
                inject_before(
                    &read_zip_entry_for_test(&path, "[Content_Types].xml")?,
                    "</Types>",
                    r#"<Override PartName="/xl/tables/table1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml"/>"#,
                ),
            ),
            (
                "xl/worksheets/sheet1.xml".to_string(),
                inject_before(
                    &sheet_xml,
                    "</worksheet>",
                    r#"<tableParts count="1"><tablePart r:id="rId1"/></tableParts>"#,
                ),
            ),
            (
                "xl/worksheets/_rels/sheet1.xml.rels".to_string(),
                concat!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
                    r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/table" Target="../tables/table1.xml"/>"#,
                    r#"</Relationships>"#
                )
                .to_string(),
            ),
            (
                "xl/tables/table1.xml".to_string(),
                concat!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                    r#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="1" name="SalesTable" displayName="SalesTable" ref="A1:B2" totalsRowCount="0">"#,
                    r#"<autoFilter ref="A1:B2"/>"#,
                    r#"<tableColumns count="2">"#,
                    r#"<tableColumn id="1" name="Quarter"/>"#,
                    r#"<tableColumn id="2" name="Revenue" totalsRowFunction="sum"/>"#,
                    r#"</tableColumns>"#,
                    r#"<tableStyleInfo name="TableStyleMedium2" showFirstColumn="0" showLastColumn="0" showRowStripes="1" showColumnStripes="0"/>"#,
                    r#"</table>"#
                )
                .to_string(),
            ),
        ]),
    )?;

    let restored = SpreadsheetArtifact::from_source_file(&path, None)?;
    let restored_sheet = restored.get_sheet(Some("Sheet1"), None).expect("sheet");
    assert_eq!(restored_sheet.tables.len(), 1);
    assert_eq!(restored_sheet.tables[0].name, "SalesTable".to_string());
    assert_eq!(
        restored_sheet.tables[0].style_name,
        Some("TableStyleMedium2".to_string())
    );
    assert_eq!(
        restored_sheet.tables[0].columns,
        vec![
            crate::SpreadsheetTableColumn {
                id: 1,
                name: "Quarter".to_string(),
                totals_row_label: None,
                totals_row_function: None,
            },
            crate::SpreadsheetTableColumn {
                id: 2,
                name: "Revenue".to_string(),
                totals_row_label: None,
                totals_row_function: Some("sum".to_string()),
            },
        ]
    );
    Ok(())
}

#[test]
fn native_xlsx_conditional_format_import_reconstructs_rules_and_differential_styles()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("native-conditional-format.xlsx");

    let mut artifact = SpreadsheetArtifact::new(Some("Native Conditional".to_string()));
    let sheet = artifact.create_sheet("Sheet1".to_string())?;
    sheet.set_value(
        CellAddress::parse("A1")?,
        Some(SpreadsheetCellValue::Integer(5)),
    )?;
    sheet.set_value(
        CellAddress::parse("A2")?,
        Some(SpreadsheetCellValue::Integer(15)),
    )?;
    sheet.set_value(
        CellAddress::parse("B1")?,
        Some(SpreadsheetCellValue::Integer(1)),
    )?;
    sheet.set_value(
        CellAddress::parse("B2")?,
        Some(SpreadsheetCellValue::Integer(10)),
    )?;
    artifact.export(&path)?;

    let styles_xml = read_zip_entry_for_test(&path, "xl/styles.xml")?;
    let sheet_xml = read_zip_entry_for_test(&path, "xl/worksheets/sheet1.xml")?;
    rewrite_xlsx_entries(
        &path,
        &BTreeMap::from([
            (
                "xl/styles.xml".to_string(),
                styles_xml.replacen(
                    r#"<dxfs count="0"></dxfs>"#,
                    concat!(
                        r#"<dxfs count="1">"#,
                        r#"<dxf><fill><patternFill patternType="solid"><fgColor rgb="FFFFC7CE"/></patternFill></fill></dxf>"#,
                        r#"</dxfs>"#
                    ),
                    1,
                ),
            ),
            (
                "xl/worksheets/sheet1.xml".to_string(),
                inject_before(
                    &sheet_xml,
                    "</worksheet>",
                    concat!(
                        r#"<conditionalFormatting sqref="A1:A2">"#,
                        r#"<cfRule type="expression" dxfId="0" priority="1" stopIfTrue="1"><formula>A1&gt;10</formula></cfRule>"#,
                        r#"</conditionalFormatting>"#,
                        r#"<conditionalFormatting sqref="B1:B2">"#,
                        r#"<cfRule type="colorScale" priority="2"><colorScale>"#,
                        r#"<cfvo type="min"/><cfvo type="max"/>"#,
                        r#"<color rgb="FFFF7128"/><color rgb="FF63BE7B"/>"#,
                        r#"</colorScale></cfRule>"#,
                        r#"</conditionalFormatting>"#
                    ),
                ),
            ),
        ]),
    )?;

    let restored = SpreadsheetArtifact::from_source_file(&path, None)?;
    let restored_sheet = restored.get_sheet(Some("Sheet1"), None).expect("sheet");
    assert_eq!(
        restored_sheet.conditional_formats,
        vec![
            crate::SpreadsheetConditionalFormat {
                id: 1,
                range: "A1:A2".to_string(),
                rule_type: crate::SpreadsheetConditionalFormatType::Expression,
                operator: None,
                formulas: vec!["A1>10".to_string()],
                text: None,
                dxf_id: Some(1),
                stop_if_true: true,
                priority: 1,
                rank: None,
                percent: None,
                time_period: None,
                above_average: None,
                equal_average: None,
                color_scale: None,
                data_bar: None,
                icon_set: None,
            },
            crate::SpreadsheetConditionalFormat {
                id: 2,
                range: "B1:B2".to_string(),
                rule_type: crate::SpreadsheetConditionalFormatType::ColorScale,
                operator: None,
                formulas: Vec::new(),
                text: None,
                dxf_id: None,
                stop_if_true: false,
                priority: 2,
                rank: None,
                percent: None,
                time_period: None,
                above_average: None,
                equal_average: None,
                color_scale: Some(crate::SpreadsheetColorScale {
                    min_type: Some("min".to_string()),
                    mid_type: None,
                    max_type: Some("max".to_string()),
                    min_value: None,
                    mid_value: None,
                    max_value: None,
                    min_color: "#FF7128".to_string(),
                    mid_color: None,
                    max_color: "#63BE7B".to_string(),
                }),
                data_bar: None,
                icon_set: None,
            },
        ]
    );
    assert_eq!(
        restored.get_differential_format(1),
        Some(&crate::SpreadsheetDifferentialFormat {
            text_style_id: None,
            fill_id: Some(1),
            border_id: None,
            alignment: None,
            number_format_id: None,
            wrap_text: None,
        })
    );
    Ok(())
}

#[test]
fn native_xlsx_chart_import_reconstructs_chart_metadata() -> Result<(), Box<dyn std::error::Error>>
{
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("native-chart.xlsx");

    let mut artifact = SpreadsheetArtifact::new(Some("Native Chart".to_string()));
    let sheet = artifact.create_sheet("Sheet1".to_string())?;
    sheet.set_value(
        CellAddress::parse("A1")?,
        Some(SpreadsheetCellValue::String("Quarter".to_string())),
    )?;
    sheet.set_value(
        CellAddress::parse("B1")?,
        Some(SpreadsheetCellValue::String("Revenue".to_string())),
    )?;
    sheet.set_value(
        CellAddress::parse("C1")?,
        Some(SpreadsheetCellValue::String("Forecast".to_string())),
    )?;
    sheet.set_value(
        CellAddress::parse("A2")?,
        Some(SpreadsheetCellValue::String("Q1".to_string())),
    )?;
    sheet.set_value(
        CellAddress::parse("B2")?,
        Some(SpreadsheetCellValue::Integer(120)),
    )?;
    sheet.set_value(
        CellAddress::parse("C2")?,
        Some(SpreadsheetCellValue::Integer(118)),
    )?;
    sheet.set_value(
        CellAddress::parse("A3")?,
        Some(SpreadsheetCellValue::String("Q2".to_string())),
    )?;
    sheet.set_value(
        CellAddress::parse("B3")?,
        Some(SpreadsheetCellValue::Integer(134)),
    )?;
    sheet.set_value(
        CellAddress::parse("C3")?,
        Some(SpreadsheetCellValue::Integer(130)),
    )?;
    artifact.export(&path)?;

    rewrite_xlsx_entries(
        &path,
        &BTreeMap::from([
            (
                "xl/worksheets/sheet1.xml".to_string(),
                inject_before(
                    &read_zip_entry_for_test(&path, "xl/worksheets/sheet1.xml")?,
                    "</worksheet>",
                    r#"<drawing r:id="rId1"/>"#,
                ),
            ),
            (
                "xl/worksheets/_rels/sheet1.xml.rels".to_string(),
                concat!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
                    r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/>"#,
                    r#"</Relationships>"#
                )
                .to_string(),
            ),
            (
                "xl/drawings/drawing1.xml".to_string(),
                concat!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                    r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
                    r#"<xdr:twoCellAnchor>"#,
                    r#"<xdr:graphicFrame>"#,
                    r#"<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart">"#,
                    r#"<c:chart r:id="rId1"/>"#,
                    r#"</a:graphicData></a:graphic>"#,
                    r#"</xdr:graphicFrame>"#,
                    r#"</xdr:twoCellAnchor>"#,
                    r#"</xdr:wsDr>"#
                )
                .to_string(),
            ),
            (
                "xl/drawings/_rels/drawing1.xml.rels".to_string(),
                concat!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
                    r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart1.xml"/>"#,
                    r#"</Relationships>"#
                )
                .to_string(),
            ),
            (
                "xl/charts/chart1.xml".to_string(),
                concat!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                    r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">"#,
                    r#"<c:style val="102"/>"#,
                    r#"<c:chart>"#,
                    r#"<c:title><c:tx><c:rich><a:p xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:r><a:t>Quarterly Revenue</a:t></a:r></a:p></c:rich></c:tx></c:title>"#,
                    r#"<c:plotArea><c:lineChart>"#,
                    r#"<c:ser><c:idx val="1"/><c:order val="0"/><c:tx><c:v>Revenue</c:v></c:tx><c:cat><c:strRef><c:f>Sheet1!$A$1:$A$3</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Sheet1!$B$1:$B$3</c:f></c:numRef></c:val></c:ser>"#,
                    r#"<c:ser><c:idx val="2"/><c:order val="1"/><c:tx><c:v>Forecast</c:v></c:tx><c:cat><c:strRef><c:f>Sheet1!$A$1:$A$3</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Sheet1!$C$1:$C$3</c:f></c:numRef></c:val></c:ser>"#,
                    r#"</c:lineChart>"#,
                    r#"<c:catAx><c:numFmt formatCode="General" sourceLinked="1"/></c:catAx>"#,
                    r#"<c:valAx><c:numFmt formatCode="General" sourceLinked="1"/></c:valAx>"#,
                    r#"</c:plotArea>"#,
                    r#"<c:legend><c:legendPos val="r"/><c:overlay val="0"/></c:legend>"#,
                    r#"<c:dispBlanksAs val="gap"/>"#,
                    r#"</c:chart>"#,
                    r#"</c:chartSpace>"#
                )
                .to_string(),
            ),
        ]),
    )?;

    let restored = SpreadsheetArtifact::from_source_file(&path, None)?;
    let restored_sheet = restored.get_sheet(Some("Sheet1"), None).expect("sheet");
    assert_eq!(restored_sheet.charts.len(), 1);
    let chart = &restored_sheet.charts[0];
    assert_eq!(chart.chart_type, crate::SpreadsheetChartType::Line);
    assert_eq!(chart.title, Some("Quarterly Revenue".to_string()));
    assert_eq!(chart.source_sheet_name, Some("Sheet1".to_string()));
    assert_eq!(chart.source_range, Some("A1:C3".to_string()));
    assert_eq!(
        chart.legend.position,
        crate::SpreadsheetChartLegendPosition::Right
    );
    assert_eq!(chart.series.len(), 2);
    assert_eq!(chart.series[0].name, Some("Revenue".to_string()));
    assert_eq!(chart.series[1].value_range, "C1:C3".to_string());
    Ok(())
}

#[test]
fn native_xlsx_pivot_import_reconstructs_pivot_metadata() -> Result<(), Box<dyn std::error::Error>>
{
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("native-pivot.xlsx");

    let mut artifact = SpreadsheetArtifact::new(Some("Native Pivot".to_string()));
    let sheet = artifact.create_sheet("Sheet1".to_string())?;
    sheet.set_value(
        CellAddress::parse("A1")?,
        Some(SpreadsheetCellValue::String("Quarter".to_string())),
    )?;
    sheet.set_value(
        CellAddress::parse("B1")?,
        Some(SpreadsheetCellValue::String("Revenue".to_string())),
    )?;
    sheet.set_value(
        CellAddress::parse("A2")?,
        Some(SpreadsheetCellValue::String("Q1".to_string())),
    )?;
    sheet.set_value(
        CellAddress::parse("B2")?,
        Some(SpreadsheetCellValue::Integer(120)),
    )?;
    artifact.export(&path)?;

    rewrite_xlsx_entries(
        &path,
        &BTreeMap::from([
            (
                "xl/workbook.xml".to_string(),
                inject_before(
                    &read_zip_entry_for_test(&path, "xl/workbook.xml")?,
                    "</workbook>",
                    r#"<pivotCaches><pivotCache cacheId="7" r:id="rId3"/></pivotCaches>"#,
                ),
            ),
            (
                "xl/_rels/workbook.xml.rels".to_string(),
                inject_before(
                    &read_zip_entry_for_test(&path, "xl/_rels/workbook.xml.rels")?,
                    "</Relationships>",
                    r#"<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotCacheDefinition" Target="pivotCache/pivotCacheDefinition1.xml"/>"#,
                ),
            ),
            (
                "xl/worksheets/sheet1.xml".to_string(),
                inject_before(
                    &read_zip_entry_for_test(&path, "xl/worksheets/sheet1.xml")?,
                    "</worksheet>",
                    r#"<pivotTableParts count="1"><pivotTablePart r:id="rId1"/></pivotTableParts>"#,
                ),
            ),
            (
                "xl/worksheets/_rels/sheet1.xml.rels".to_string(),
                concat!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
                    r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotTable" Target="../pivotTables/pivotTable1.xml"/>"#,
                    r#"</Relationships>"#
                )
                .to_string(),
            ),
            (
                "xl/pivotCache/pivotCacheDefinition1.xml".to_string(),
                concat!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                    r#"<pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
                    r#"<cacheFields count="2">"#,
                    r#"<cacheField name="Quarter"/>"#,
                    r#"<cacheField name="Revenue"/>"#,
                    r#"</cacheFields>"#,
                    r#"</pivotCacheDefinition>"#
                )
                .to_string(),
            ),
            (
                "xl/pivotTables/pivotTable1.xml".to_string(),
                concat!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                    r#"<pivotTableDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="SalesPivot" cacheId="7">"#,
                    r#"<location ref="E3:G8"/>"#,
                    r#"<pivotFields count="2">"#,
                    r#"<pivotField axis="axisRow"><items count="1"><item t="default" x="0"/></items></pivotField>"#,
                    r#"<pivotField><items count="1"><item t="default" x="0"/></items></pivotField>"#,
                    r#"</pivotFields>"#,
                    r#"<rowFields count="1"><field x="0"/></rowFields>"#,
                    r#"<dataFields count="1"><dataField fld="1" subtotal="sum" name="Sum of Revenue"/></dataFields>"#,
                    r#"<filters count="1"><filter fld="0" type="captionEqual"/></filters>"#,
                    r#"<pivotTableStyleInfo name="PivotStyleLight16"/>"#,
                    r#"</pivotTableDefinition>"#
                )
                .to_string(),
            ),
        ]),
    )?;

    let restored = SpreadsheetArtifact::from_source_file(&path, None)?;
    let restored_sheet = restored.get_sheet(Some("Sheet1"), None).expect("sheet");
    assert_eq!(restored_sheet.pivot_tables.len(), 1);
    let pivot = &restored_sheet.pivot_tables[0];
    assert_eq!(pivot.name, "SalesPivot".to_string());
    assert_eq!(pivot.cache_id, 7);
    assert_eq!(pivot.address, Some("E3:G8".to_string()));
    assert_eq!(
        pivot.row_fields,
        vec![crate::SpreadsheetPivotFieldReference {
            field_index: 0,
            field_name: Some("Quarter".to_string()),
        }]
    );
    assert_eq!(
        pivot.data_fields,
        vec![crate::SpreadsheetPivotDataField {
            field_index: 1,
            field_name: Some("Revenue".to_string()),
            name: Some("Sum of Revenue".to_string()),
            subtotal: Some("sum".to_string()),
        }]
    );
    assert_eq!(pivot.style_name, Some("PivotStyleLight16".to_string()));
    Ok(())
}

#[test]
fn clearing_range_formulas_removes_stale_cached_values() -> Result<(), Box<dyn std::error::Error>> {
    let mut sheet = SpreadsheetSheet::new("Sheet1".to_string());
    let range = CellRange::parse("A1:B1")?;

    sheet.set_values_matrix(
        &range,
        &[vec![
            Some(SpreadsheetCellValue::Integer(1)),
            Some(SpreadsheetCellValue::Integer(2)),
        ]],
    )?;
    sheet.set_formulas_matrix(&range, &[vec![None, None]])?;
    assert_eq!(sheet.cells, BTreeMap::new());

    sheet.set_range_to_value(&range, Some(SpreadsheetCellValue::Integer(9)))?;
    sheet.set_range_to_formula(&range, None)?;
    assert_eq!(sheet.cells, BTreeMap::new());
    Ok(())
}

#[test]
fn style_validation_rejects_missing_references_and_unknown_style_indices()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let mut manager = SpreadsheetArtifactManager::default();
    let created = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: None,
            action: "create".to_string(),
            args: serde_json::json!({ "name": "Style Validation" }),
        },
        temp_dir.path(),
    )?;
    let artifact_id = created.artifact_id;

    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_sheet".to_string(),
            args: serde_json::json!({ "name": "Sheet1" }),
        },
        temp_dir.path(),
    )?;

    let invalid_cell_format = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_cell_format".to_string(),
            args: serde_json::json!({
                "format": {
                    "text_style_id": 999
                }
            }),
        },
        temp_dir.path(),
    );
    assert_eq!(
        invalid_cell_format.unwrap_err().to_string(),
        "invalid args for action `create_cell_format`: text style `999` was not found"
    );

    let invalid_dxf = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_differential_format".to_string(),
            args: serde_json::json!({
                "format": {
                    "fill_id": 999
                }
            }),
        },
        temp_dir.path(),
    );
    assert_eq!(
        invalid_dxf.unwrap_err().to_string(),
        "invalid args for action `create_differential_format`: fill `999` was not found"
    );

    let invalid_style_assignment = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id),
            action: "set_cell_style".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "address": "A1",
                "style_index": 999
            }),
        },
        temp_dir.path(),
    );
    assert_eq!(
        invalid_style_assignment.unwrap_err().to_string(),
        "invalid args for action `set_cell_style`: style index `999` was not found"
    );
    Ok(())
}

#[test]
fn builtin_number_formats_are_normalized_consistently() -> Result<(), Box<dyn std::error::Error>> {
    let mut artifact = SpreadsheetArtifact::new(Some("Number Formats".to_string()));

    let by_id = artifact.create_number_format(
        SpreadsheetNumberFormat {
            format_id: Some(4),
            format_code: Some("not-the-builtin-code".to_string()),
        },
        None,
        false,
    )?;
    assert_eq!(
        artifact.get_number_format(by_id),
        Some(&SpreadsheetNumberFormat {
            format_id: Some(4),
            format_code: Some("#,##0.00".to_string()),
        })
    );

    let by_code = artifact.create_number_format(
        SpreadsheetNumberFormat {
            format_id: None,
            format_code: Some("0.00%".to_string()),
        },
        None,
        false,
    )?;
    assert_eq!(
        artifact.get_number_format(by_code),
        Some(&SpreadsheetNumberFormat {
            format_id: Some(10),
            format_code: Some("0.00%".to_string()),
        })
    );
    Ok(())
}

fn read_zip_entry_for_test(
    path: &std::path::Path,
    entry_name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut entry = archive.by_name(entry_name)?;
    let mut contents = String::new();
    use std::io::Read as _;
    entry.read_to_string(&mut contents)?;
    Ok(contents)
}

fn rewrite_xlsx_entries(
    path: &std::path::Path,
    replacements: &BTreeMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let temp_path = path.with_extension("rewritten.xlsx");
    let source = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(source)?;
    let target = std::fs::File::create(&temp_path)?;
    let mut writer = zip::ZipWriter::new(target);
    let options = zip::write::SimpleFileOptions::default();
    let mut seen = std::collections::BTreeSet::new();

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_string();
        seen.insert(name.clone());
        writer.start_file(&name, options)?;
        if let Some(replacement) = replacements.get(&name) {
            use std::io::Write as _;
            writer.write_all(replacement.as_bytes())?;
        } else {
            std::io::copy(&mut entry, &mut writer)?;
        }
    }

    for (name, replacement) in replacements {
        if seen.contains(name) {
            continue;
        }
        writer.start_file(name, options)?;
        use std::io::Write as _;
        writer.write_all(replacement.as_bytes())?;
    }

    writer.finish()?;
    std::fs::rename(temp_path, path)?;
    Ok(())
}

fn inject_before(original: &str, marker: &str, addition: &str) -> String {
    original.replacen(marker, &format!("{addition}{marker}"), 1)
}

#[test]
fn manager_supports_bulk_sizes_and_row_heights() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let mut manager = SpreadsheetArtifactManager::default();
    let created = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: None,
            action: "create".to_string(),
            args: serde_json::json!({ "name": "Sizing" }),
        },
        temp_dir.path(),
    )?;
    let artifact_id = created.artifact_id;

    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_sheet".to_string(),
            args: serde_json::json!({ "name": "Sheet1" }),
        },
        temp_dir.path(),
    )?;

    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "set_column_widths_bulk".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "widths": {
                    "A:B": 12.0,
                    "D": 20.0
                }
            }),
        },
        temp_dir.path(),
    )?;
    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "set_row_height".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "row_index": 2,
                "height": 18.0
            }),
        },
        temp_dir.path(),
    )?;
    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "set_row_heights".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "start_row_index": 3,
                "end_row_index": 4,
                "height": 21.0
            }),
        },
        temp_dir.path(),
    )?;
    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "set_row_heights_bulk".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "heights": {
                    "4": null,
                    "5": 25.0
                }
            }),
        },
        temp_dir.path(),
    )?;

    let row_height = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "get_row_height".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "row_index": 5
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(row_height.row_height, Some(25.0));

    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "cleanup_and_validate_sheet".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1"
            }),
        },
        temp_dir.path(),
    )?;

    let sheet = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id),
            action: "get_sheet".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1"
            }),
        },
        temp_dir.path(),
    )?;
    let restored: SpreadsheetSheet =
        serde_json::from_value(sheet.serialized_dict.expect("sheet dict"))?;
    assert_eq!(
        restored.column_widths,
        BTreeMap::from([(1, 12.0), (2, 12.0), (4, 20.0)])
    );
    assert_eq!(
        restored.row_heights,
        BTreeMap::from([(2, 18.0), (3, 21.0), (5, 25.0)])
    );
    Ok(())
}

#[test]
fn manager_style_registry_and_format_summaries_work() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let mut manager = SpreadsheetArtifactManager::default();
    let created = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: None,
            action: "create".to_string(),
            args: serde_json::json!({ "name": "Styles" }),
        },
        temp_dir.path(),
    )?;
    let artifact_id = created.artifact_id;

    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_sheet".to_string(),
            args: serde_json::json!({ "name": "Sheet1" }),
        },
        temp_dir.path(),
    )?;

    let text_style = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_text_style".to_string(),
            args: serde_json::json!({
                "style": {
                    "bold": true,
                    "italic": true,
                    "underline": true,
                    "font_size": 14.0,
                    "font_color": "#112233",
                    "text_alignment": "center",
                    "anchor": "middle",
                    "vertical_text_orientation": "stacked",
                    "text_rotation": 90,
                    "paragraph_spacing": true,
                    "bottom_inset": 1.0,
                    "left_inset": 2.0,
                    "right_inset": 3.0,
                    "top_inset": 4.0,
                    "font_family": "IBM Plex Sans",
                    "font_scheme": "minor",
                    "typeface": "IBM Plex Sans",
                    "font_face": {
                        "font_family": "IBM Plex Sans",
                        "font_scheme": "minor",
                        "typeface": "IBM Plex Sans"
                    }
                }
            }),
        },
        temp_dir.path(),
    )?;
    let text_style_id = text_style.style_id.expect("text style id");

    let fill = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_fill".to_string(),
            args: serde_json::json!({
                "fill": {
                    "solid_fill_color": "#ffeeaa",
                    "pattern_type": "solid",
                    "pattern_foreground_color": "#ffeeaa",
                    "pattern_background_color": "#221100",
                    "color_transforms": ["tint:0.2"],
                    "gradient_fill_type": "linear",
                    "gradient_stops": [
                        { "position": 0.0, "color": "#ffeeaa" },
                        { "position": 1.0, "color": "#aa5500" }
                    ],
                    "gradient_kind": "linear",
                    "angle": 45.0,
                    "scaled": true,
                    "path_type": "rect",
                    "fill_rectangle": {
                        "left": 0.0,
                        "right": 1.0,
                        "top": 0.0,
                        "bottom": 1.0
                    },
                    "image_reference": "image://fill"
                }
            }),
        },
        temp_dir.path(),
    )?;
    let fill_id = fill.style_id.expect("fill id");

    let border = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_border".to_string(),
            args: serde_json::json!({
                "border": {
                    "top": { "style": "solid", "color": "#111111" },
                    "right": { "style": "dashed", "color": "#222222" },
                    "bottom": { "style": "double", "color": "#333333" },
                    "left": { "style": "solid", "color": "#444444" }
                }
            }),
        },
        temp_dir.path(),
    )?;
    let border_id = border.style_id.expect("border id");

    let number_format = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_number_format".to_string(),
            args: serde_json::json!({
                "number_format": {
                    "format_id": 4
                }
            }),
        },
        temp_dir.path(),
    )?;
    let number_format_id = number_format.style_id.expect("number format id");

    let base_format = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_cell_format".to_string(),
            args: serde_json::json!({
                "format": {
                    "text_style_id": text_style_id,
                    "number_format_id": number_format_id,
                    "alignment": {
                        "horizontal": "center",
                        "vertical": "middle"
                    }
                }
            }),
        },
        temp_dir.path(),
    )?;
    let base_format_id = base_format.style_id.expect("base format id");

    let derived_format = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_cell_format".to_string(),
            args: serde_json::json!({
                "format": {
                    "fill_id": fill_id,
                    "border_id": border_id,
                    "wrap_text": true,
                    "base_cell_style_format_id": base_format_id
                }
            }),
        },
        temp_dir.path(),
    )?;
    let derived_format_id = derived_format.style_id.expect("derived format id");

    let merged_format = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_cell_format".to_string(),
            args: serde_json::json!({
                "source_format_id": derived_format_id,
                "merge_with_existing_components": true,
                "format": {
                    "alignment": {
                        "vertical": "bottom"
                    }
                }
            }),
        },
        temp_dir.path(),
    )?;
    let merged_format_id = merged_format.style_id.expect("merged format id");

    let differential_format = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_differential_format".to_string(),
            args: serde_json::json!({
                "format": {
                    "fill_id": fill_id,
                    "wrap_text": true
                }
            }),
        },
        temp_dir.path(),
    )?;
    let differential_format_id = differential_format.style_id.expect("dxf id");

    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "set_cell_style".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "address": "A1",
                "style_index": merged_format_id
            }),
        },
        temp_dir.path(),
    )?;

    let summary = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "get_cell_format_summary".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "address": "A1"
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(
        summary.cell_format_summary,
        Some(SpreadsheetCellFormatSummary {
            style_index: merged_format_id,
            text_style: Some(SpreadsheetTextStyle {
                bold: Some(true),
                italic: Some(true),
                underline: Some(true),
                font_size: Some(14.0),
                font_color: Some("#112233".to_string()),
                text_alignment: Some("center".to_string()),
                anchor: Some("middle".to_string()),
                vertical_text_orientation: Some("stacked".to_string()),
                text_rotation: Some(90),
                paragraph_spacing: Some(true),
                bottom_inset: Some(1.0),
                left_inset: Some(2.0),
                right_inset: Some(3.0),
                top_inset: Some(4.0),
                font_family: Some("IBM Plex Sans".to_string()),
                font_scheme: Some("minor".to_string()),
                typeface: Some("IBM Plex Sans".to_string()),
                font_face: Some(SpreadsheetFontFace {
                    font_family: Some("IBM Plex Sans".to_string()),
                    font_scheme: Some("minor".to_string()),
                    typeface: Some("IBM Plex Sans".to_string()),
                }),
            }),
            fill: Some(SpreadsheetFill {
                solid_fill_color: Some("#ffeeaa".to_string()),
                pattern_type: Some("solid".to_string()),
                pattern_foreground_color: Some("#ffeeaa".to_string()),
                pattern_background_color: Some("#221100".to_string()),
                color_transforms: vec!["tint:0.2".to_string()],
                gradient_fill_type: Some("linear".to_string()),
                gradient_stops: vec![
                    crate::SpreadsheetGradientStop {
                        position: 0.0,
                        color: "#ffeeaa".to_string(),
                    },
                    crate::SpreadsheetGradientStop {
                        position: 1.0,
                        color: "#aa5500".to_string(),
                    },
                ],
                gradient_kind: Some("linear".to_string()),
                angle: Some(45.0),
                scaled: Some(true),
                path_type: Some("rect".to_string()),
                fill_rectangle: Some(crate::SpreadsheetFillRectangle {
                    left: 0.0,
                    right: 1.0,
                    top: 0.0,
                    bottom: 1.0,
                }),
                image_reference: Some("image://fill".to_string()),
            }),
            border: Some(crate::SpreadsheetBorder {
                top: Some(crate::SpreadsheetBorderLine {
                    style: Some("solid".to_string()),
                    color: Some("#111111".to_string()),
                }),
                right: Some(crate::SpreadsheetBorderLine {
                    style: Some("dashed".to_string()),
                    color: Some("#222222".to_string()),
                }),
                bottom: Some(crate::SpreadsheetBorderLine {
                    style: Some("double".to_string()),
                    color: Some("#333333".to_string()),
                }),
                left: Some(crate::SpreadsheetBorderLine {
                    style: Some("solid".to_string()),
                    color: Some("#444444".to_string()),
                }),
            }),
            alignment: Some(crate::SpreadsheetAlignment {
                horizontal: Some("center".to_string()),
                vertical: Some("bottom".to_string()),
            }),
            number_format: Some(SpreadsheetNumberFormat {
                format_id: Some(4),
                format_code: Some("#,##0.00".to_string()),
            }),
            wrap_text: Some(true),
        })
    );

    let retrieved_format = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "get_cell_format".to_string(),
            args: serde_json::json!({ "id": merged_format_id }),
        },
        temp_dir.path(),
    )?;
    let retrieved_format: SpreadsheetCellFormat =
        serde_json::from_value(retrieved_format.serialized_dict.expect("cell format"))?;
    assert_eq!(
        retrieved_format,
        SpreadsheetCellFormat {
            text_style_id: None,
            fill_id: Some(fill_id),
            border_id: Some(border_id),
            alignment: Some(crate::SpreadsheetAlignment {
                horizontal: None,
                vertical: Some("bottom".to_string()),
            }),
            number_format_id: None,
            wrap_text: Some(true),
            base_cell_style_format_id: Some(base_format_id),
        }
    );

    let retrieved_number_format = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "get_number_format".to_string(),
            args: serde_json::json!({ "id": number_format_id }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(
        serde_json::from_value::<SpreadsheetNumberFormat>(
            retrieved_number_format
                .serialized_dict
                .expect("number format")
        )?,
        SpreadsheetNumberFormat {
            format_id: Some(4),
            format_code: Some("#,##0.00".to_string()),
        }
    );

    let retrieved_text_style = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "get_text_style".to_string(),
            args: serde_json::json!({ "id": text_style_id }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(
        serde_json::from_value::<SpreadsheetTextStyle>(
            retrieved_text_style.serialized_dict.expect("text style")
        )?
        .bold,
        Some(true)
    );

    let range_summary = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "get_range_format_summary".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "range": "A1:B2"
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(range_summary.top_left_style_index, Some(merged_format_id));
    assert_eq!(
        range_summary
            .range_format
            .as_ref()
            .map(|format| format.range.clone()),
        Some("A1:B2".to_string())
    );

    let retrieved_dxf = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id),
            action: "get_differential_format".to_string(),
            args: serde_json::json!({ "id": differential_format_id }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(
        serde_json::from_value::<crate::SpreadsheetDifferentialFormat>(
            retrieved_dxf.serialized_dict.expect("differential format")
        )?
        .wrap_text,
        Some(true)
    );
    Ok(())
}

#[test]
fn sheet_references_resolve_cells_and_ranges() -> Result<(), Box<dyn std::error::Error>> {
    let sheet = SpreadsheetSheet::new("Sheet1".to_string());
    assert_eq!(
        sheet.reference("A1")?,
        SpreadsheetSheetReference::Cell {
            cell_ref: sheet.cell_ref("A1")?,
        }
    );
    assert_eq!(
        sheet.reference("A1:B2")?,
        SpreadsheetSheetReference::Range {
            range_ref: sheet.range_ref("A1:B2")?,
        }
    );
    Ok(())
}

#[test]
fn manager_get_reference_and_xlsx_import_preserve_workbook_name()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("named.xlsx");

    let mut artifact = SpreadsheetArtifact::new(Some("Named Workbook".to_string()));
    artifact.create_sheet("Sheet1".to_string())?.set_value(
        CellAddress::parse("A1")?,
        Some(SpreadsheetCellValue::Integer(9)),
    )?;
    artifact.export(&path)?;

    let restored = SpreadsheetArtifact::from_source_file(&path, None)?;
    assert_eq!(restored.name, Some("Named Workbook".to_string()));

    let mut manager = SpreadsheetArtifactManager::default();
    let imported = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: None,
            action: "read".to_string(),
            args: serde_json::json!({ "path": path }),
        },
        temp_dir.path(),
    )?;
    let artifact_id = imported.artifact_id;

    let cell_reference = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "get_reference".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "reference": "A1"
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(
        cell_reference
            .raw_cell
            .as_ref()
            .and_then(|cell| cell.value.clone()),
        Some(SpreadsheetCellValue::Integer(9))
    );

    let range_reference = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id),
            action: "get_reference".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "reference": "A1:B2"
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(
        range_reference
            .range_ref
            .as_ref()
            .map(|range_ref| range_ref.address.clone()),
        Some("A1:B2".to_string())
    );
    Ok(())
}

#[test]
fn manager_render_actions_support_workbook_sheet_and_range()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let mut manager = SpreadsheetArtifactManager::default();
    let created = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: None,
            action: "create".to_string(),
            args: serde_json::json!({ "name": "Render" }),
        },
        temp_dir.path(),
    )?;
    let artifact_id = created.artifact_id;

    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "create_sheet".to_string(),
            args: serde_json::json!({ "name": "Sheet1" }),
        },
        temp_dir.path(),
    )?;
    manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "set_range_values".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "range": "A1:C4",
                "values": [
                    ["h1", "h2", "h3"],
                    ["a", 1, 2],
                    ["b", 3, 4],
                    ["c", 5, 6]
                ]
            }),
        },
        temp_dir.path(),
    )?;

    let workbook = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "render_workbook".to_string(),
            args: serde_json::json!({
                "output_path": temp_dir.path().join("workbook-previews"),
                "include_headers": false
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(workbook.exported_paths.len(), 1);
    assert!(workbook.exported_paths[0].exists());

    let sheet = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id.clone()),
            action: "render_sheet".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "output_path": temp_dir.path().join("sheet-preview.html"),
                "center_address": "B3",
                "width": 220,
                "height": 90,
                "scale": 1.5,
                "performance_mode": true
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(sheet.exported_paths.len(), 1);
    assert!(sheet.exported_paths[0].exists());
    assert!(
        sheet
            .rendered_html
            .as_ref()
            .is_some_and(|html| html.contains("data-performance-mode=\"true\""))
    );

    let range = manager.execute(
        SpreadsheetArtifactRequest {
            artifact_id: Some(artifact_id),
            action: "render_range".to_string(),
            args: serde_json::json!({
                "sheet_name": "Sheet1",
                "range": "A2:C4",
                "output_path": temp_dir.path().join("range-preview.html"),
                "include_headers": true
            }),
        },
        temp_dir.path(),
    )?;
    assert_eq!(range.exported_paths.len(), 1);
    assert_eq!(
        range
            .range_ref
            .as_ref()
            .map(|range_ref| range_ref.address.clone()),
        Some("A2:C4".to_string())
    );
    assert!(
        range
            .rendered_html
            .as_ref()
            .is_some_and(|html| html.contains("<th>A</th>"))
    );
    Ok(())
}
