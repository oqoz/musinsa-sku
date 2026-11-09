use std::{collections::BTreeSet, path::Path};

use anyhow::Result;
use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder, Workbook, Worksheet};
use serde::Serialize;
use serde_json::Value;

use crate::{model::*, pipeline::Report, summary};

fn header_format() -> Format {
    Format::new()
        .set_bold()
        .set_background_color(Color::RGB(0xD9D9D9))
        .set_align(FormatAlign::Center)
        .set_align(FormatAlign::VerticalCenter)
        .set_text_wrap()
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0x999999))
}

fn cell_format() -> Format {
    Format::new()
        .set_align(FormatAlign::Center)
        .set_align(FormatAlign::VerticalCenter)
        .set_text_wrap()
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0x999999))
}

fn raw_sheet<T: Serialize>(
    book: &mut Workbook,
    name: &str,
    columns: &[&str],
    rows: &[T],
) -> Result<()> {
    let sheet = book.add_worksheet();
    sheet.set_name(name)?;
    sheet.set_freeze_panes(1, 0)?;
    let mut widths: Vec<_> = columns.iter().map(|s| s.len().min(45)).collect();
    for (col, header) in columns.iter().enumerate() {
        sheet.write_string_with_format(0, col as u16, *header, &header_format())?;
    }
    for (row, item) in rows.iter().enumerate() {
        let record = serde_json::to_value(item)?;
        for (col, key) in columns.iter().enumerate() {
            let value = &record[*key];
            let row = row as u32 + 1;
            let col = col as u16;
            match value {
                Value::Null => {}
                Value::Number(n) => {
                    sheet.write_number(row, col, n.as_f64().unwrap_or_default())?;
                }
                Value::Bool(b) => {
                    sheet.write_boolean(row, col, *b)?;
                }
                Value::String(s) => {
                    // Explicit string writes preserve leading zeros and never interpret source text as formulas.
                    sheet.write_string(row, col, s)?;
                    let width = s
                        .chars()
                        .map(|c| if c.is_ascii() { 1 } else { 2 })
                        .sum::<usize>();
                    widths[col as usize] = widths[col as usize].max(width.min(45));
                }
                _ => {
                    sheet.write_string(row, col, value.to_string())?;
                }
            }
        }
    }
    for (col, width) in widths.into_iter().enumerate() {
        sheet.set_column_width(col as u16, (width + 2).clamp(10, 45) as f64)?;
    }
    sheet.autofilter(0, 0, rows.len() as u32, columns.len() as u16 - 1)?;
    Ok(())
}

fn merged_text(
    sheet: &mut Worksheet,
    first: u32,
    last: u32,
    col: u16,
    value: &str,
    format: &Format,
) -> Result<()> {
    if last > first {
        sheet.merge_range(first, col, last, col, value, format)?;
    } else {
        sheet.write_string_with_format(first, col, value, format)?;
    }
    Ok(())
}

fn merged_number(
    sheet: &mut Worksheet,
    first: u32,
    last: u32,
    col: u16,
    value: f64,
    format: &Format,
) -> Result<()> {
    if last > first {
        sheet.merge_range(first, col, last, col, "", format)?;
    }
    sheet.write_number_with_format(first, col, value, format)?;
    Ok(())
}

fn price_summary(prices: &[i64]) -> String {
    fn commas(n: i64) -> String {
        let s = n.to_string();
        s.chars()
            .enumerate()
            .map(|(i, c)| {
                if i > 0 && (s.len() - i).is_multiple_of(3) {
                    format!(",{c}")
                } else {
                    c.to_string()
                }
            })
            .collect()
    }
    if prices.is_empty() {
        return "-".into();
    }
    let average =
        (prices.iter().map(|p| *p as f64).sum::<f64>() / prices.len() as f64).round() as i64;
    format!(
        "{} / {} / {}원",
        commas(*prices.iter().max().unwrap()),
        commas(*prices.iter().min().unwrap()),
        commas(average)
    )
}

fn summary_sheet(book: &mut Workbook, report: &Report) -> Result<()> {
    let sheet = book.add_worksheet();
    sheet.set_name("SKU_요약")?;
    sheet.set_freeze_panes(1, 0)?;
    sheet.set_row_height(0, 30)?;
    let headers = [
        "카테고리",
        "중분류",
        "모델명",
        "색상",
        "컬러 수",
        "사이즈 수",
        "모델 SKU 개수",
        "SKU 개수",
        "SKU 비중",
        "최고/최저/평균가",
        "베이직/뉴베이직/트렌디",
        "수집 상태",
    ];
    for (col, name) in headers.iter().enumerate() {
        sheet.write_string_with_format(0, col as u16, *name, &header_format())?;
    }
    for (col, width) in [18, 18, 38, 16, 8, 8, 14, 12, 12, 28, 22, 32]
        .into_iter()
        .enumerate()
    {
        sheet.set_column_width(col as u16, width)?;
    }
    let center = cell_format();
    let left = center.clone().set_align(FormatAlign::Left);
    let percent = center.clone().set_num_format("0.00%");
    let groups = summary::model_groups(&report.products, &report.skus);
    let total_skus: usize = groups.iter().map(|g| g.sku_keys.len()).sum();
    let mut majors: Vec<_> = report
        .category_tree
        .iter()
        .map(|c| c.major_name.clone())
        .collect();
    for group in &groups {
        if !majors.contains(&group.major) {
            majors.push(group.major.clone());
        }
    }
    let mut row = 1_u32;
    for major in majors {
        let major_groups: Vec<_> = groups.iter().filter(|g| g.major == major).collect();
        if major_groups.is_empty() {
            continue;
        }
        let major_start = row;
        let major_total: usize = major_groups.iter().map(|g| g.sku_keys.len()).sum();
        let mut categories: Vec<_> = report
            .category_tree
            .iter()
            .find(|c| c.major_name == major)
            .map(|c| c.subs.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default();
        for group in &major_groups {
            if !categories.contains(&group.category) {
                categories.push(group.category.clone());
            }
        }
        for category in categories {
            let category_groups: Vec<_> = major_groups
                .iter()
                .filter(|g| g.category == category)
                .collect();
            if category_groups.is_empty() {
                continue;
            }
            let start = row;
            let count: usize = category_groups.iter().map(|g| g.sku_keys.len()).sum();
            let prices: Vec<_> = category_groups
                .iter()
                .flat_map(|g| g.prices.iter().copied())
                .collect();
            for group in category_groups {
                let first = row;
                for color in &group.colors {
                    sheet.write_string_with_format(
                        row,
                        3,
                        if color.is_empty() { "미확인" } else { color },
                        &center,
                    )?;
                    row += 1;
                }
                let last = row - 1;
                merged_text(sheet, first, last, 2, &group.name, &left)?;
                merged_number(
                    sheet,
                    first,
                    last,
                    4,
                    group.colors.iter().filter(|c| !c.is_empty()).count() as f64,
                    &center,
                )?;
                merged_number(sheet, first, last, 5, group.sizes.len() as f64, &center)?;
                // Count only existing product/color/size tuples, never color_count * size_count.
                merged_number(sheet, first, last, 6, group.sku_keys.len() as f64, &center)?;
                merged_text(sheet, first, last, 11, &group.status_label(), &left)?;
            }
            let last = row - 1;
            merged_text(sheet, start, last, 1, &category, &center)?;
            merged_number(sheet, start, last, 7, count as f64, &center)?;
            merged_number(
                sheet,
                start,
                last,
                8,
                if total_skus == 0 {
                    0.0
                } else {
                    count as f64 / total_skus as f64
                },
                &percent,
            )?;
            merged_text(sheet, start, last, 9, &price_summary(&prices), &center)?;
            merged_text(sheet, start, last, 10, "미분류: 100%", &center)?;
        }
        let pct = if total_skus == 0 {
            0.0
        } else {
            major_total as f64 / total_skus as f64 * 100.0
        };
        merged_text(
            sheet,
            major_start,
            row - 1,
            0,
            &format!("{major}\n(확인된 SKU: {major_total} / {pct:.1}%)"),
            &center,
        )?;
    }
    if !report.complete_for_requested_scope {
        sheet.set_tab_color(Color::RGB(0xD97706));
        // Report completeness is visible even when category discovery produced no product rows.
        if row == 1 {
            sheet.write_string_with_format(1, 11, "수집 실패 — collection_issues 확인", &left)?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct CategoryRow<'a> {
    major_category: &'a str,
    major_code: &'a str,
    category_code: &'a str,
    raw_category: &'a str,
}

#[derive(Serialize)]
struct Metric {
    key: String,
    value: String,
}

pub fn save(path: &Path, report: &Report) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut book = Workbook::new();
    summary_sheet(&mut book, report)?;
    raw_sheet(
        &mut book,
        "model_summary",
        &[
            "major_category",
            "raw_category",
            "product_id",
            "product_name",
            "color_count",
            "size_count",
            "sku_count",
            "available_sku_count",
            "soldout_sku_count",
            "price",
            "product_url",
            "collection_status",
        ],
        &summary::product_summaries(&report.products, &report.skus),
    )?;
    raw_sheet(
        &mut book,
        "products_raw",
        &[
            "crawl_date",
            "gender_filter",
            "brand",
            "major_category",
            "raw_category",
            "category_code",
            "product_id",
            "product_name",
            "product_url",
            "price",
            "discount_rate",
            "image_url",
            "is_soldout",
            "source_category_url",
            "collection_status",
            "collection_error",
        ],
        &report.products,
    )?;
    raw_sheet(
        &mut book,
        "options_raw",
        &[
            "product_id",
            "product_name",
            "major_category",
            "raw_category",
            "color",
            "size",
            "option_name",
            "option_status",
            "option_price",
            "stock_qty",
            "product_url",
        ],
        &report.options,
    )?;
    raw_sheet(
        &mut book,
        "sku_result",
        &[
            "major_category",
            "raw_category",
            "product_id",
            "product_name",
            "color",
            "size",
            "sku_key",
            "sku_status",
            "product_url",
        ],
        &report.skus,
    )?;
    let categories: Vec<_> = report
        .category_tree
        .iter()
        .flat_map(|c| {
            c.subs.iter().map(move |s| CategoryRow {
                major_category: &c.major_name,
                major_code: &c.major_code,
                category_code: &s.code,
                raw_category: &s.name,
            })
        })
        .collect();
    raw_sheet(
        &mut book,
        "category_master",
        &[
            "major_category",
            "major_code",
            "category_code",
            "raw_category",
        ],
        &categories,
    )?;
    raw_sheet(
        &mut book,
        "collection_issues",
        &["stage", "key", "message"],
        &report.issues,
    )?;
    let mut metrics = Vec::new();
    let mut metric = |key: &str, value: String| {
        metrics.push(Metric {
            key: key.into(),
            value,
        })
    };
    metric(
        "요청 범위 수집 완료",
        report.complete_for_requested_scope.to_string(),
    );
    metric(
        "요청 범위 SKU 수집 완료",
        report.sku_collection_complete.to_string(),
    );
    metric(
        "카테고리 필터 없는 브랜드 전체 조회",
        report.scope.all_products.to_string(),
    );
    metric(
        "선택 카테고리 (빈 목록: 전체)",
        format!("{:?}", report.scope.selected_categories),
    );
    metric(
        "카테고리별 상품 제한 (0: 전체)",
        report.scope.per_category_product_limit.to_string(),
    );
    metric(
        "전체 상품 제한",
        format!("{:?}", report.scope.total_product_limit),
    );
    metric("옵션 수집 생략", report.scope.skip_options.to_string());
    metric("상품 수", report.products.len().to_string());
    metric("확인된 SKU 수", report.skus.len().to_string());
    metric(
        "실패 상품 수",
        report
            .products
            .iter()
            .filter(|p| p.collection_status == CollectionStatus::Failed)
            .count()
            .to_string(),
    );
    metric(
        "옵션 없음 상품 수",
        report
            .products
            .iter()
            .filter(|p| p.collection_status == CollectionStatus::Empty)
            .count()
            .to_string(),
    );
    metric(
        "상품 없음/단종 수",
        report
            .products
            .iter()
            .filter(|p| p.collection_status == CollectionStatus::Unavailable)
            .count()
            .to_string(),
    );
    metric("오류 수", report.issues.len().to_string());
    metric("시작 UTC", report.started_at.clone());
    metric("이 기록의 측정 시점 UTC", report.finished_at.clone());
    metric(
        "이 기록의 경과 시간 (초)",
        format!("{:.3}", report.wall_seconds),
    );
    metric(
        "시간 기록 안내",
        "실시간 수집 시 이 시트는 엑셀 저장 직전 작성. 최종 총 시간은 .run.json 참조".into(),
    );
    metric(
        "SKU 집계 단위",
        "실제 (상품 ID, 색상, 사이즈) 조합. 같은 표시 모델명의 서로 다른 상품 ID는 보존".into(),
    );
    metric(
        "모델 그룹 기준",
        "대분류 + 중분류 + 색상 suffix를 제외한 상품명. 공식 모델 ID가 아님".into(),
    );
    metric(
        "비중 기준",
        "확인된 SKU만 분모에 포함. 부분 실패 시 전체 상품 현황으로 해석하지 않음".into(),
    );
    let hosts: BTreeSet<_> = report
        .http_requests
        .iter()
        .map(|r| r.host.as_str())
        .collect();
    for host in hosts {
        for status in report
            .http_requests
            .iter()
            .filter(|r| r.host == host)
            .map(|r| r.status)
            .collect::<BTreeSet<_>>()
        {
            metric(
                &format!("HTTP {host} {status:?}"),
                report
                    .http_requests
                    .iter()
                    .filter(|r| r.host == host && r.status == status)
                    .count()
                    .to_string(),
            );
        }
    }
    for stage in &report.stages {
        metric(
            &format!("단계 {} (초)", stage.name),
            format!("{:.3}", stage.seconds),
        );
    }
    raw_sheet(&mut book, "run_summary", &["key", "value"], &metrics)?;
    let temp = path.with_extension(format!("tmp-{}.xlsx", std::process::id()));
    book.save(&temp)?;
    std::fs::rename(temp, path)?;
    Ok(())
}
