use std::{collections::BTreeSet, time::Instant};

use anyhow::{Context, Result, ensure};
use chrono::Utc;
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};

use crate::{
    api::{self, ApiClient, HttpRecord},
    categories,
    config::{Cli, Config, OutputFormat},
    csv_export,
    model::*,
    summary,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stage {
    pub name: String,
    pub seconds: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    #[serde(default)]
    pub all_products: bool,
    pub selected_categories: Vec<String>,
    pub total_product_limit: Option<usize>,
    pub per_category_product_limit: usize,
    pub skip_options: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub started_at: String,
    pub finished_at: String,
    pub wall_seconds: f64,
    pub config: Config,
    pub scope: Scope,
    pub complete_for_requested_scope: bool,
    pub sku_collection_complete: bool,
    pub listing_candidates: usize,
    pub category_tree: Vec<Category>,
    pub products: Vec<ProductRow>,
    pub options: Vec<OptionRow>,
    pub skus: Vec<SkuRow>,
    pub issues: Vec<Issue>,
    pub stages: Vec<Stage>,
    pub http_requests: Vec<HttpRecord>,
}

impl Report {
    pub fn new(config: Config) -> Self {
        Self {
            schema_version: 1,
            started_at: Utc::now().to_rfc3339(),
            finished_at: String::new(),
            wall_seconds: 0.0,
            scope: Scope {
                all_products: true,
                selected_categories: vec![],
                total_product_limit: None,
                per_category_product_limit: 0,
                skip_options: false,
            },
            config,
            complete_for_requested_scope: false,
            sku_collection_complete: false,
            listing_candidates: 0,
            category_tree: vec![],
            products: vec![],
            options: vec![],
            skus: vec![],
            issues: vec![],
            stages: vec![],
            http_requests: vec![],
        }
    }

    pub fn stage(&mut self, name: &str, started: Instant) {
        let seconds = started.elapsed().as_secs_f64();
        println!("[{name}] {seconds:.3}s");
        self.stages.push(Stage {
            name: name.into(),
            seconds,
        });
    }

    pub fn update_completeness(&mut self) {
        self.complete_for_requested_scope = self.issues.is_empty()
            && (self.scope.all_products || !self.category_tree.is_empty())
            && self.products.iter().all(|p| {
                if self.scope.skip_options {
                    p.collection_status == CollectionStatus::Skipped
                } else {
                    matches!(
                        p.collection_status,
                        CollectionStatus::Complete
                            | CollectionStatus::Empty
                            | CollectionStatus::Unavailable
                    )
                }
            });
        self.sku_collection_complete =
            self.complete_for_requested_scope && !self.scope.skip_options;
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "Unsupported snapshot version");
        let ids: BTreeSet<_> = self
            .products
            .iter()
            .map(|p| p.product_id.as_str())
            .collect();
        ensure!(
            ids.len() == self.products.len(),
            "Duplicate product IDs in snapshot"
        );
        for sku in &self.skus {
            ensure!(
                ids.contains(sku.product_id.as_str()),
                "SKU references an unknown product: {}",
                sku.product_id
            );
        }
        let sku_keys: BTreeSet<_> = self
            .skus
            .iter()
            .map(|s| (&s.product_id, &s.color, &s.size))
            .collect();
        ensure!(
            sku_keys.len() == self.skus.len(),
            "Duplicate SKU combinations in snapshot"
        );
        for option in &self.options {
            ensure!(
                ids.contains(option.product_id.as_str()),
                "Option references an unknown product: {}",
                option.product_id
            );
        }
        Ok(())
    }
}

/// Development utility for converting an existing snapshot. Never used by normal collection.
pub fn export_saved(source: &std::path::Path, output: &std::path::Path) -> Result<bool> {
    let mut report: Report = serde_json::from_slice(
        &std::fs::read(source)
            .with_context(|| format!("Reading snapshot: {}", source.display()))?,
    )?;
    report.validate()?;
    report.config.output = output.into();
    report.config.format = OutputFormat::Csv;
    report.config.csv_bom = true;
    report.config.validate_output()?;
    report.update_completeness();
    csv_export::save(output, &report)?;
    Ok(report.complete_for_requested_scope)
}

pub async fn run(cli: Cli) -> Result<bool> {
    collect(Config::from_cli(&cli)?, api::LISTING_URL, api::OPTIONS_URL).await
}

/// The endpoint arguments allow the same complete flow to be tested against local fixtures.
/// The application always supplies the fixed production endpoints above.
pub async fn collect(mut config: Config, listing_url: &str, options_url: &str) -> Result<bool> {
    let started = Instant::now();
    config.format = OutputFormat::Csv;
    config.csv_bom = true;
    config.max_products = 0;
    config.skip_options = false;
    config.validate()?;
    csv_export::check_destination(&config.output)?;
    println!(
        "Musinsa Standard Woman: all products, options and SKUs -> CSV\nOutput: {}\nRequest interval: {}ms/host | Option concurrency: {} | Request timeout: {}s",
        config.output.display(),
        config.request_interval_ms,
        config.detail_concurrency,
        config.timeout_seconds
    );
    let client = ApiClient::new(&config)?;
    let mut report = Report::new(config.clone());
    let stage = Instant::now();
    let result = api::fetch_listing(&client, WorkUnit::all_products(), 0, listing_url).await;
    println!(
        "Full listing: {} products{}",
        result.products.len(),
        if result.error.is_some() {
            " (partial failure)"
        } else {
            ""
        }
    );
    if let Some(message) = result.error {
        report.issues.push(Issue {
            stage: "listing".into(),
            key: "all_products".into(),
            message,
        });
    }
    report.listing_candidates = result.products.len();
    report.products = summary::deduplicate_products(result.products);
    report.stage("listing", stage);
    println!("Products after deduplication: {}", report.products.len());

    let stage = Instant::now();
    let count = report.products.len();
    let mut results = stream::iter(std::mem::take(&mut report.products))
        .map(|product| api::fetch_detail(&client, product, options_url))
        .buffer_unordered(config.detail_concurrency);
    let mut done = 0;
    let mut failed = 0;
    while let Some((product, options, skus)) = results.next().await {
        done += 1;
        if product.collection_status == CollectionStatus::Failed {
            failed += 1;
            report.issues.push(Issue {
                stage: "detail".into(),
                key: product.product_id.clone(),
                message: product.collection_error.clone().unwrap_or_default(),
            });
        }
        report.products.push(product);
        report.options.extend(options);
        report.skus.extend(skus);
        if done % 25 == 0 || done == count {
            println!(
                "Options [{done}/{count}] failed: {failed} | SKUs: {}",
                report.skus.len()
            );
        }
    }
    report
        .products
        .sort_by(|a, b| a.product_id.cmp(&b.product_id));
    report.options.sort_by(|a, b| {
        (&a.product_id, &a.color, &a.size).cmp(&(&b.product_id, &b.color, &b.size))
    });
    report.skus.sort_by(|a, b| {
        (&a.product_id, &a.color, &a.size).cmp(&(&b.product_id, &b.color, &b.size))
    });
    report.stage("detail", stage);
    report.http_requests = client.records();
    report.update_completeness();
    report.validate()?;
    report.wall_seconds = started.elapsed().as_secs_f64();
    report.finished_at = Utc::now().to_rfc3339();
    let json_path = config.output.join("run.json");
    let stage = Instant::now();
    if let Err(error) = csv_export::save(&config.output, &report) {
        let diagnostic = config.output.with_extension("failed.run.json");
        if let Err(diagnostic_error) = categories::write_json(&diagnostic, &report) {
            eprintln!("Failed to save diagnostic record: {diagnostic_error}");
        }
        return Err(error.context(format!(
            "Export failed. Collection diagnostic record: {}",
            diagnostic.display()
        )));
    }
    report.stage("csv", stage);
    report.wall_seconds = started.elapsed().as_secs_f64();
    report.finished_at = Utc::now().to_rfc3339();
    csv_export::refresh_metadata(&config.output, &report)?;
    println!(
        "Products: {} | Options: {} | SKUs: {} | Issues: {}\nTotal: {:.3}s | {}\nOutput: {}\nRun record: {}",
        report.products.len(),
        report.options.len(),
        report.skus.len(),
        report.issues.len(),
        report.wall_seconds,
        if report.complete_for_requested_scope {
            "Collection complete"
        } else {
            "Partial collection - check collection_issues.csv"
        },
        config.output.display(),
        json_path.display()
    );
    Ok(report.complete_for_requested_scope)
}
