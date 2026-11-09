//! One CSV per logical sheet, published as a complete output directory.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{categories::write_json, model::*, pipeline::Report, summary};

const GENERATOR: &str = "musinsa-sku-csv";
pub const TABLE_NAMES: &[&str] = &[
    "sku_summary",
    "category_summary",
    "model_summary",
    "products_raw",
    "options_raw",
    "sku_result",
    "category_master",
    "collection_issues",
    "run_summary",
];

type Schema = &'static [(&'static str, &'static str, bool)];

const PRODUCTS: Schema = &[
    ("crawl_date", "text", false),
    ("gender_filter", "text", false),
    ("brand", "text", false),
    ("major_category", "text", false),
    ("raw_category", "text", false),
    ("category_code", "text", false),
    ("product_id", "text", false),
    ("product_name", "text", false),
    ("product_url", "text", false),
    ("price", "integer", true),
    ("discount_rate", "text", true),
    ("image_url", "text", false),
    ("is_soldout", "text", false),
    ("source_category_url", "text", false),
    ("collection_status", "text", false),
    ("collection_error", "text", true),
];
const OPTIONS: Schema = &[
    ("product_id", "text", false),
    ("product_name", "text", false),
    ("major_category", "text", false),
    ("raw_category", "text", false),
    ("color", "text", false),
    ("size", "text", false),
    ("option_name", "text", false),
    ("option_status", "text", false),
    ("option_price", "integer", true),
    ("stock_qty", "integer", true),
    ("product_url", "text", false),
];
const SKUS: Schema = &[
    ("major_category", "text", false),
    ("raw_category", "text", false),
    ("product_id", "text", false),
    ("product_name", "text", false),
    ("color", "text", false),
    ("size", "text", false),
    ("sku_key", "text", false),
    ("sku_status", "text", false),
    ("product_url", "text", false),
];
const PRODUCT_SUMMARY: Schema = &[
    ("major_category", "text", false),
    ("raw_category", "text", false),
    ("product_id", "text", false),
    ("product_name", "text", false),
    ("color_count", "integer", false),
    ("size_count", "integer", false),
    ("sku_count", "integer", false),
    ("available_sku_count", "integer", false),
    ("soldout_sku_count", "integer", false),
    ("price", "integer", true),
    ("product_url", "text", false),
    ("collection_status", "text", false),
];
const MODELS: Schema = &[
    ("카테고리", "text", false),
    ("중분류", "text", false),
    ("모델명", "text", false),
    ("상품 ID 목록", "json_array", false),
    ("색상 목록", "json_array", false),
    ("사이즈 목록", "json_array", false),
    ("상품 수", "integer", false),
    ("컬러 수", "integer", false),
    ("사이즈 수", "integer", false),
    ("모델 SKU 개수", "integer", false),
    ("최저가", "integer", true),
    ("최고가", "integer", true),
    ("평균가", "number", true),
    ("수집 상태", "text", false),
    ("옵션 수집 완료", "boolean", false),
];
const CATEGORY_SUMMARY: Schema = &[
    ("major_category", "text", false),
    ("raw_category", "text", false),
    ("product_count", "integer", false),
    ("known_sku_count", "integer", false),
    ("known_sku_share", "number", false),
    ("min_price", "integer", true),
    ("max_price", "integer", true),
    ("average_price", "number", true),
    ("failed_product_count", "integer", false),
    ("sku_collection_complete", "boolean", false),
];
const CATEGORY_MASTER: Schema = &[
    ("major_category", "text", false),
    ("major_code", "text", false),
    ("category_code", "text", false),
    ("raw_category", "text", false),
];
const ISSUES: Schema = &[
    ("stage", "text", false),
    ("key", "text", false),
    ("message", "text", false),
];
const METRICS: Schema = &[("key", "text", false), ("value", "text", false)];

#[derive(Debug, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Table {
    pub name: String,
    pub file: String,
    pub rows: usize,
    pub bytes: u64,
    pub columns: Vec<Column>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub generator: String,
    pub schema_version: u32,
    pub brand: String,
    pub gender_filter: String,
    pub exported_at: String,
    pub collection_started_at: String,
    pub collection_finished_at: String,
    pub complete_for_requested_scope: bool,
    pub sku_collection_complete: bool,
    pub encoding: String,
    pub utf8_bom: bool,
    pub delimiter: String,
    pub line_ending: String,
    pub null_value: String,
    pub crawl_date_timezone: String,
    pub tables: Vec<Table>,
}

fn table<T: Serialize>(
    root: &Path,
    name: &str,
    schema: Schema,
    rows: impl IntoIterator<Item = T>,
    bom: bool,
) -> Result<Table> {
    let filename = format!("{name}.csv");
    let destination = root.join(&filename);
    let mut temp = tempfile::NamedTempFile::new_in(root)?;
    if bom {
        temp.write_all(b"\xEF\xBB\xBF")?;
    }
    let mut count = 0;
    {
        let mut writer = csv::WriterBuilder::new()
            .has_headers(false)
            .terminator(csv::Terminator::CRLF)
            .from_writer(temp.as_file_mut());
        writer.write_record(schema.iter().map(|c| c.0))?;
        for row in rows {
            writer.serialize(row)?;
            count += 1;
        }
        writer.flush()?;
    }
    temp.persist(&destination)
        .with_context(|| format!("Saving CSV: {}", destination.display()))?;
    Ok(Table {
        name: name.into(),
        file: filename,
        rows: count,
        bytes: fs::metadata(destination)?.len(),
        columns: schema
            .iter()
            .map(|(name, data_type, nullable)| Column {
                name: (*name).into(),
                data_type: (*data_type).into(),
                nullable: *nullable,
            })
            .collect(),
    })
}

#[derive(Serialize)]
struct ModelRow {
    major: String,
    category: String,
    model: String,
    product_ids: String,
    colors: String,
    sizes: String,
    product_count: usize,
    color_count: usize,
    size_count: usize,
    sku_count: usize,
    min_price: Option<i64>,
    max_price: Option<i64>,
    average_price: Option<f64>,
    status: String,
    complete: bool,
}

fn option_status_complete(status: CollectionStatus) -> bool {
    matches!(
        status,
        CollectionStatus::Complete | CollectionStatus::Empty | CollectionStatus::Unavailable
    )
}

fn average(prices: &[i64]) -> Option<f64> {
    (!prices.is_empty())
        .then(|| prices.iter().map(|p| *p as f64).sum::<f64>() / prices.len() as f64)
}

fn models(report: &Report) -> Result<Vec<ModelRow>> {
    summary::model_groups(&report.products, &report.skus)
        .into_iter()
        .map(|g| {
            let colors: Vec<_> = g.colors.iter().filter(|c| !c.is_empty()).collect();
            Ok(ModelRow {
                major: g.major.clone(),
                category: g.category.clone(),
                model: g.name.clone(),
                product_ids: serde_json::to_string(&g.product_ids)?,
                colors: serde_json::to_string(&colors)?,
                sizes: serde_json::to_string(&g.sizes)?,
                product_count: g.product_ids.len(),
                color_count: colors.len(),
                size_count: g.sizes.len(),
                sku_count: g.sku_keys.len(),
                min_price: g.prices.iter().copied().min(),
                max_price: g.prices.iter().copied().max(),
                average_price: average(&g.prices),
                status: g.status_label(),
                complete: g.statuses.keys().copied().all(option_status_complete),
            })
        })
        .collect()
}

#[derive(Serialize, Default)]
struct CategorySummary {
    major_category: String,
    raw_category: String,
    product_count: usize,
    known_sku_count: usize,
    known_sku_share: f64,
    min_price: Option<i64>,
    max_price: Option<i64>,
    average_price: Option<f64>,
    failed_product_count: usize,
    sku_collection_complete: bool,
}

fn category_summaries(report: &Report) -> Vec<CategorySummary> {
    let mut grouped: BTreeMap<(&str, &str), Vec<&ProductRow>> = BTreeMap::new();
    let mut sku_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut known_skus = BTreeSet::new();
    for sku in &report.skus {
        if known_skus.insert((&sku.product_id, &sku.color, &sku.size)) {
            *sku_counts.entry(&sku.product_id).or_default() += 1;
        }
    }
    for p in &report.products {
        grouped
            .entry((&p.major_category, &p.raw_category))
            .or_default()
            .push(p);
    }
    grouped
        .into_iter()
        .map(|((major, raw), products)| {
            let prices: Vec<_> = products.iter().filter_map(|p| p.price).collect();
            let count = products
                .iter()
                .map(|p| sku_counts.get(p.product_id.as_str()).copied().unwrap_or(0))
                .sum();
            CategorySummary {
                major_category: major.into(),
                raw_category: raw.into(),
                product_count: products.len(),
                known_sku_count: count,
                known_sku_share: if known_skus.is_empty() {
                    0.0
                } else {
                    count as f64 / known_skus.len() as f64
                },
                min_price: prices.iter().copied().min(),
                max_price: prices.iter().copied().max(),
                average_price: average(&prices),
                failed_product_count: products
                    .iter()
                    .filter(|p| p.collection_status == CollectionStatus::Failed)
                    .count(),
                sku_collection_complete: report.sku_collection_complete
                    && products
                        .iter()
                        .all(|p| option_status_complete(p.collection_status)),
            }
        })
        .collect()
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

fn metrics(report: &Report) -> Vec<Metric> {
    let mut rows = Vec::new();
    let mut add = |key: &str, value: String| {
        rows.push(Metric {
            key: key.into(),
            value,
        })
    };
    add(
        "complete_for_requested_scope",
        report.complete_for_requested_scope.to_string(),
    );
    add(
        "sku_collection_complete",
        report.sku_collection_complete.to_string(),
    );
    add("started_at", report.started_at.clone());
    add("finished_at", report.finished_at.clone());
    add("wall_seconds", format!("{:.6}", report.wall_seconds));
    add("all_products", report.scope.all_products.to_string());
    add(
        "selected_categories",
        serde_json::to_string(&report.scope.selected_categories).expect("strings"),
    );
    add(
        "total_product_limit",
        report
            .scope
            .total_product_limit
            .map(|n| n.to_string())
            .unwrap_or_default(),
    );
    add(
        "per_category_product_limit",
        report.scope.per_category_product_limit.to_string(),
    );
    add("skip_options", report.scope.skip_options.to_string());
    add("products", report.products.len().to_string());
    add("options", report.options.len().to_string());
    add("skus", report.skus.len().to_string());
    add("issues", report.issues.len().to_string());
    for status in [
        CollectionStatus::Pending,
        CollectionStatus::Complete,
        CollectionStatus::Empty,
        CollectionStatus::Unavailable,
        CollectionStatus::Failed,
        CollectionStatus::Skipped,
    ] {
        add(
            &format!(
                "products_{}",
                serde_json::to_value(status)
                    .expect("enum")
                    .as_str()
                    .expect("string")
            ),
            report
                .products
                .iter()
                .filter(|p| p.collection_status == status)
                .count()
                .to_string(),
        );
    }
    let mut statuses = BTreeMap::new();
    for r in &report.http_requests {
        *statuses
            .entry(format!(
                "http_{}_{}",
                r.host,
                r.status
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "error".into())
            ))
            .or_insert(0) += 1;
    }
    for (name, count) in statuses {
        add(&name, count.to_string());
    }
    for stage in &report.stages {
        add(
            &format!("stage_{}_seconds", stage.name),
            format!("{:.6}", stage.seconds),
        );
    }
    add(
        "sku_identity",
        "product_id + color + size; existing combinations only".into(),
    );
    add(
        "share_denominator",
        "known SKU tuples only; check completeness before interpreting as full assortment".into(),
    );
    add("timing_boundary", "final metadata write excluded".into());
    rows
}

fn manifest(report: &Report, tables: Vec<Table>) -> Manifest {
    Manifest {
        generator: GENERATOR.into(),
        schema_version: 1,
        brand: crate::config::BRAND.into(),
        gender_filter: "F".into(),
        exported_at: Utc::now().to_rfc3339(),
        collection_started_at: report.started_at.clone(),
        collection_finished_at: report.finished_at.clone(),
        complete_for_requested_scope: report.complete_for_requested_scope,
        sku_collection_complete: report.sku_collection_complete,
        encoding: "UTF-8".into(),
        utf8_bom: report.config.csv_bom,
        delimiter: ",".into(),
        line_ending: "CRLF".into(),
        null_value: String::new(),
        crawl_date_timezone: "Asia/Seoul (UTC+09:00)".into(),
        tables,
    }
}

pub fn check_destination(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "CSV output path must be a regular directory: {}",
        path.display()
    );
    let canonical = path.canonicalize()?;
    ensure!(
        !std::env::current_dir()?
            .canonicalize()?
            .starts_with(&canonical),
        "Cannot use the current project or a parent directory as the CSV output path"
    );
    let entries = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
    if entries.is_empty() {
        return Ok(());
    }
    let old: Manifest = serde_json::from_slice(&fs::read(path.join("manifest.json")).context(
        "Existing directory has no CSV output manifest. Use an empty directory or a new output path",
    )?)?;
    ensure!(
        old.generator == GENERATOR && old.schema_version == 1,
        "Cannot replace a directory created by another program"
    );
    let mut expected: BTreeSet<_> = TABLE_NAMES.iter().map(|n| format!("{n}.csv")).collect();
    // Allow replacing bundles created before the summary filename was translated.
    if old
        .tables
        .iter()
        .any(|t| t.name == "SKU_요약" && t.file == "SKU_요약.csv")
    {
        expected.insert("SKU_요약.csv".into());
    }
    expected.extend(["run.json".into(), "manifest.json".into()]);
    for entry in entries {
        ensure!(
            entry.file_type()?.is_file()
                && expected.contains(&entry.file_name().to_string_lossy().to_string()),
            "Output directory contains an extra file: {}. Choose a different output path",
            entry.path().display()
        );
    }
    Ok(())
}

/// Replace only directories created by this exporter; keep unrelated user files intact.
pub fn save(path: &Path, report: &Report) -> Result<()> {
    report.validate()?;
    check_destination(path)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".musinsa-csv-")
        .tempdir_in(parent)?;
    let root = staging.path();
    let bom = report.config.csv_bom;
    let categories = report.category_tree.iter().flat_map(|c| {
        c.subs.iter().map(move |s| CategoryRow {
            major_category: &c.major_name,
            major_code: &c.major_code,
            category_code: &s.code,
            raw_category: &s.name,
        })
    });
    let tables = vec![
        table(root, "sku_summary", MODELS, models(report)?, bom)?,
        table(
            root,
            "category_summary",
            CATEGORY_SUMMARY,
            category_summaries(report),
            bom,
        )?,
        table(
            root,
            "model_summary",
            PRODUCT_SUMMARY,
            summary::product_summaries(&report.products, &report.skus),
            bom,
        )?,
        table(root, "products_raw", PRODUCTS, &report.products, bom)?,
        table(root, "options_raw", OPTIONS, &report.options, bom)?,
        table(root, "sku_result", SKUS, &report.skus, bom)?,
        table(root, "category_master", CATEGORY_MASTER, categories, bom)?,
        table(root, "collection_issues", ISSUES, &report.issues, bom)?,
        table(root, "run_summary", METRICS, metrics(report), bom)?,
    ];
    write_json(&root.join("run.json"), report)?;
    write_json(&root.join("manifest.json"), &manifest(report, tables))?;
    if path.exists() {
        // Validate again immediately before replacing an existing generated directory.
        check_destination(path)?;
        let backup = tempfile::Builder::new()
            .prefix(".musinsa-csv-old-")
            .tempdir_in(parent)?;
        let old = backup.path().join("previous");
        fs::rename(path, &old)?;
        if let Err(error) = fs::rename(root, path) {
            if let Err(rollback) = fs::rename(&old, path) {
                let kept = backup.keep();
                bail!(
                    "Failed to replace output: {error}; rollback failed: {rollback}; previous data: {}",
                    kept.display()
                );
            }
            return Err(error.into());
        }
    } else {
        fs::rename(root, path)?;
    }
    Ok(())
}

/// Update final timing metadata without serializing the data tables a second time.
pub fn refresh_metadata(path: &Path, report: &Report) -> Result<()> {
    let mut data: Manifest = serde_json::from_slice(&fs::read(path.join("manifest.json"))?)?;
    ensure!(
        data.generator == GENERATOR && data.collection_started_at == report.started_at,
        "Another run has changed the output directory"
    );
    let run_table = table(
        path,
        "run_summary",
        METRICS,
        metrics(report),
        report.config.csv_bom,
    )?;
    let slot = data
        .tables
        .iter_mut()
        .find(|t| t.name == "run_summary")
        .context("Missing run_summary schema")?;
    *slot = run_table;
    data.exported_at = Utc::now().to_rfc3339();
    data.collection_finished_at = report.finished_at.clone();
    data.complete_for_requested_scope = report.complete_for_requested_scope;
    data.sku_collection_complete = report.sku_collection_complete;
    write_json(&path.join("run.json"), report)?;
    write_json(&path.join("manifest.json"), &data)?;
    Ok(())
}
