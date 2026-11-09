use std::path::PathBuf;

use anyhow::{Result, ensure};
use clap::Parser;
use serde::{Deserialize, Serialize};

pub const BRAND: &str = "musinsastandardwoman";
pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:124.0) Gecko/20100101 Firefox/124.0";
pub const BRAND_URL: &str = "https://www.musinsa.com/brand/musinsastandardwoman/products";

// Retained for reading old snapshots and the development-only XLSX comparison.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Csv,
    Xlsx,
}

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Collect all Musinsa Standard Woman products and options as CSV"
)]
pub struct Cli {
    /// Directory for output CSV files
    #[arg(long, default_value = "outputs/musinsa_all")]
    pub output: PathBuf,
    /// Timeout per HTTP request in seconds, not for the entire collection
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u64).range(1..=300))]
    pub timeout: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub output: PathBuf,
    pub format: OutputFormat,
    pub csv_bom: bool,
    pub max_products: usize,
    pub max_scrolls: usize,
    pub delay: f64,
    pub headless: bool,
    pub skip_options: bool,
    pub workers: usize,
    pub browser: String,
    pub listing_concurrency: usize,
    pub listing_page_size: usize,
    pub detail_concurrency: usize,
    pub request_interval_ms: u64,
    pub max_retries: u32,
    pub timeout_seconds: u64,
    pub category_cache: PathBuf,
    pub category_cache_hours: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output: "outputs/musinsa_all".into(),
            format: OutputFormat::Csv,
            csv_bom: true,
            max_products: 0,
            max_scrolls: 80,
            delay: 0.8,
            headless: true,
            skip_options: false,
            workers: 1,
            browser: "auto".into(),
            listing_concurrency: 4,
            listing_page_size: 100,
            detail_concurrency: 8,
            request_interval_ms: 100,
            max_retries: 5,
            timeout_seconds: 20,
            category_cache: ".cache/musinsa/categories.json".into(),
            category_cache_hours: 24,
        }
    }
}

impl Config {
    pub fn from_cli(cli: &Cli) -> Result<Self> {
        let config = Self {
            output: cli.output.clone(),
            timeout_seconds: cli.timeout,
            ..Self::default()
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.browser.as_str(), "auto" | "firefox" | "chrome"),
            "browser must be auto, firefox or chrome"
        );
        ensure!(
            (1..=64).contains(&self.listing_concurrency)
                && (1..=64).contains(&self.detail_concurrency),
            "API concurrency must be between 1 and 64"
        );
        ensure!(
            (1..=100).contains(&self.listing_page_size),
            "listing_page_size must be between 1 and 100"
        );
        ensure!(
            (1..=60_000).contains(&self.request_interval_ms),
            "request_interval_ms must be between 1 and 60000"
        );
        ensure!(
            (1..=300).contains(&self.timeout_seconds),
            "timeout_seconds must be between 1 and 300"
        );
        ensure!(self.max_retries <= 10, "max_retries must be at most 10");
        ensure!(
            self.delay.is_finite() && self.delay >= 0.0,
            "delay must be a finite nonnegative number"
        );
        self.validate_output()?;
        Ok(())
    }

    pub fn validate_output(&self) -> Result<()> {
        ensure!(
            !self.output.as_os_str().is_empty(),
            "Output path must not be empty"
        );
        match self.format {
            OutputFormat::Csv => ensure!(
                !self
                    .output
                    .extension()
                    .is_some_and(|e| e == "csv" || e == "xlsx"),
                "CSV tables are saved as separate files. Use a directory such as --output outputs/musinsa_all"
            ),
            OutputFormat::Xlsx => {
                ensure!(
                    self.output.extension().is_some_and(|e| e == "xlsx"),
                    "XLSX output must have the .xlsx extension"
                );
                ensure!(!self.csv_bom, "UTF-8 BOM applies only to CSV output");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_only_sets_output_and_request_timeout_with_fixed_fast_defaults() {
        let cli = Cli::try_parse_from([
            "musinsa-sku",
            "--output",
            "outputs/custom",
            "--timeout",
            "30",
        ])
        .unwrap();
        let c = Config::from_cli(&cli).unwrap();
        assert_eq!(c.output, PathBuf::from("outputs/custom"));
        assert_eq!(c.timeout_seconds, 30);
        assert_eq!(c.request_interval_ms, 100);
        assert_eq!(c.detail_concurrency, 8);
        assert_eq!(c.listing_page_size, 100);
        assert_eq!(c.max_retries, 5);
        assert_eq!(c.max_products, 0);
        assert!(!c.skip_options);
        assert_eq!(c.format, OutputFormat::Csv);
        assert!(c.csv_bom);
    }

    #[test]
    fn default_invocation_needs_no_config_file() {
        let cli = Cli::try_parse_from(["musinsa-sku"]).unwrap();
        let c = Config::from_cli(&cli).unwrap();
        assert_eq!(c.output, PathBuf::from("outputs/musinsa_all"));
        assert_eq!(c.timeout_seconds, 20);
    }

    #[test]
    fn removed_options_are_rejected_and_timeout_is_validated() {
        for args in [
            vec!["--preset", "fast"],
            vec!["--all-products"],
            vec!["--csv-bom"],
            vec!["--format", "csv"],
            vec!["--limit", "10"],
            vec!["--max-products", "10"],
            vec!["--category", "001001"],
            vec!["--refresh-categories"],
            vec!["--config", "commands.yaml"],
            vec!["--replay", "run.json"],
            vec!["--timeout", "0"],
            vec!["--timeout", "301"],
            vec!["--timeout", "-1"],
        ] {
            assert!(Cli::try_parse_from(std::iter::once("musinsa-sku").chain(args)).is_err());
        }
    }
}
