use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail, ensure};
use chrono::{FixedOffset, Utc};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{sync::Mutex as AsyncMutex, time::Instant as AsyncInstant};

use crate::{
    config::{BRAND, Config, USER_AGENT},
    model::*,
};

pub const LISTING_URL: &str = "https://api.musinsa.com/api2/dp/v2/plp/goods";
pub const OPTIONS_URL: &str = "https://goods-detail.musinsa.com/api2/goods";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRecord {
    pub host: String,
    pub path: String,
    pub status: Option<u16>,
    pub seconds: f64,
    pub attempt: u32,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct RequestFailure {
    pub status: Option<u16>,
    pub message: String,
}
impl fmt::Display for RequestFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RequestFailure {}

#[derive(Clone)]
pub struct ApiClient {
    client: Client,
    interval: Duration,
    retries: u32,
    page_size: usize,
    gates: RateGates,
    records: Arc<Mutex<Vec<HttpRecord>>>,
}

struct RateGate {
    next: AsyncInstant,
    interval: Duration,
}

type RateGates = Arc<Mutex<BTreeMap<String, Arc<AsyncMutex<RateGate>>>>>;

impl ApiClient {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(config.timeout_seconds))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            interval: Duration::from_millis(config.request_interval_ms),
            retries: config.max_retries,
            page_size: config.listing_page_size,
            gates: Arc::default(),
            records: Arc::default(),
        })
    }

    fn gate(&self, url: &Url) -> Arc<AsyncMutex<RateGate>> {
        self.gates
            .lock()
            .unwrap()
            .entry(url.origin().ascii_serialization())
            .or_insert_with(|| {
                Arc::new(AsyncMutex::new(RateGate {
                    next: AsyncInstant::now(),
                    interval: self.interval,
                }))
            })
            .clone()
    }

    async fn wait_turn(&self, url: &Url) {
        let gate = self.gate(url);
        loop {
            let wake = {
                let mut next = gate.lock().await;
                if next.next <= AsyncInstant::now() {
                    next.next = AsyncInstant::now() + next.interval;
                    return;
                }
                next.next
            };
            // Release the lock while sleeping so a 429 can advance the shared cooldown.
            tokio::time::sleep_until(wake).await;
        }
    }

    async fn cooldown(&self, url: &Url, duration: Duration, rate_limited: bool) {
        let gate = self.gate(url);
        let mut next = gate.lock().await;
        if rate_limited && next.interval < Duration::from_millis(250) {
            next.interval = Duration::from_millis(250);
            eprintln!(
                "HTTP 429: increasing the request interval for {} to 250ms",
                url.host_str().unwrap_or("")
            );
        }
        if rate_limited {
            next.next = next.next.max(AsyncInstant::now() + next.interval);
        }
        next.next = next.next.max(AsyncInstant::now() + duration);
    }

    pub fn records(&self) -> Vec<HttpRecord> {
        self.records.lock().unwrap().clone()
    }

    pub async fn text(
        &self,
        url: &Url,
        referer: &str,
    ) -> std::result::Result<String, RequestFailure> {
        for attempt in 0..=self.retries {
            self.wait_turn(url).await;
            let started = Instant::now();
            let response = self
                .client
                .get(url.clone())
                .header("Accept", "application/json,text/html,*/*")
                .header("Origin", "https://www.musinsa.com")
                .header("Referer", referer)
                .send()
                .await;
            let (status, error, body, retry_delay) = match response {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let delay = response
                        .headers()
                        .get("Retry-After")
                        .and_then(|v| v.to_str().ok())
                        .and_then(retry_after);
                    if response.status().is_success() {
                        match response.text().await {
                            Ok(body) => (Some(status), None, Some(body), None),
                            Err(e) => (
                                Some(status),
                                Some(e.to_string()),
                                None,
                                Some(backoff(attempt)),
                            ),
                        }
                    } else {
                        let retryable = matches!(status, 408 | 425 | 429 | 500..=599);
                        (
                            Some(status),
                            Some(format!(
                                "HTTP {status}: {}{}",
                                url.host_str().unwrap_or(""),
                                url.path()
                            )),
                            None,
                            retryable.then(|| {
                                delay.unwrap_or_else(|| {
                                    if status == 429 {
                                        Duration::from_secs(60)
                                    } else {
                                        backoff(attempt)
                                    }
                                })
                            }),
                        )
                    }
                }
                Err(e) => (None, Some(e.to_string()), None, Some(backoff(attempt))),
            };
            self.records.lock().unwrap().push(HttpRecord {
                host: url.host_str().unwrap_or("").into(),
                path: url.path().into(),
                status,
                seconds: started.elapsed().as_secs_f64(),
                attempt: attempt + 1,
                error: error.clone(),
            });
            if let Some(body) = body {
                return Ok(body);
            }
            if let Some(delay) = retry_delay {
                // Shared by all workers for this origin; retries also pass through the rate gate.
                self.cooldown(url, delay, status == Some(429)).await;
                if attempt < self.retries {
                    eprintln!(
                        "Retry {}/{}: {} (waiting {:.1}s)",
                        attempt + 1,
                        self.retries,
                        error.as_deref().unwrap_or("Request failed"),
                        delay.as_secs_f64()
                    );
                    continue;
                }
            }
            return Err(RequestFailure {
                status,
                message: error.unwrap_or_else(|| "Request failed".into()),
            });
        }
        unreachable!()
    }

    pub async fn json(&self, url: &Url, referer: &str) -> Result<Value> {
        let body = self.text(url, referer).await?;
        serde_json::from_str(&body).with_context(|| {
            format!(
                "Invalid JSON response: {}{}",
                url.host_str().unwrap_or(""),
                url.path()
            )
        })
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(2_u64.saturating_pow(attempt + 1).min(60))
}

fn retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|date| date.duration_since(SystemTime::now()).unwrap_or_default())
}

pub fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => clean(s),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn number(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

fn pagination_number(data: &Value, keys: &[&str]) -> Option<usize> {
    keys.iter().find_map(|key| {
        number(&data["pagination"][key])
            .or_else(|| number(&data[key]))
            .and_then(|n| usize::try_from(n).ok())
    })
}

pub fn listing_product(item: &Value, unit: &WorkUnit) -> Result<ProductRow> {
    let id = scalar(&item["goodsNo"]);
    let name = scalar(&item["goodsName"]);
    ensure!(
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()),
        "Missing or invalid product ID"
    );
    ensure!(!name.is_empty(), "Missing name for product {id}");
    let rate = number(&item["saleRate"])
        .filter(|r| *r != 0)
        .or_else(|| number(&item["couponSaleRate"]));
    Ok(ProductRow {
        crawl_date: Utc::now()
            .with_timezone(&FixedOffset::east_opt(9 * 3600).unwrap())
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        gender_filter: "F".into(),
        brand: BRAND.into(),
        major_category: unit.major_category.clone(),
        raw_category: unit.raw_category.clone(),
        category_code: unit.category_code.clone(),
        product_url: format!("https://www.musinsa.com/products/{id}?gf=F"),
        product_id: id,
        product_name: name,
        price: number(&item["normalPrice"])
            .filter(|p| *p > 0)
            .or_else(|| number(&item["price"])),
        discount_rate: rate.map(|r| format!("{r}%")),
        image_url: scalar(&item["thumbnail"]),
        is_soldout: if item["isSoldOut"].as_bool().unwrap_or(false) {
            "품절"
        } else {
            "판매중"
        }
        .into(),
        source_category_url: category_url(&unit.category_code),
        collection_status: CollectionStatus::Pending,
        collection_error: None,
    })
}

#[derive(Debug)]
pub struct ListingResult {
    pub unit: WorkUnit,
    pub products: Vec<ProductRow>,
    pub error: Option<String>,
}

pub async fn fetch_listing(
    client: &ApiClient,
    unit: WorkUnit,
    max: usize,
    endpoint: &str,
) -> ListingResult {
    let mut products = Vec::new();
    let result: Result<()> = async {
        let page_size = client.page_size.to_string();
        let mut initial = Url::parse_with_params(
            endpoint,
            &[
                ("gf", "F"),
                ("brand", BRAND),
                ("sortCode", "NEW"),
                ("page", "1"),
                ("size", page_size.as_str()),
                ("caller", "FLAGSHIP"),
            ],
        )?;
        if !unit.category_code.is_empty() {
            initial
                .query_pairs_mut()
                .append_pair("category", &unit.category_code);
        }
        let mut url = initial.clone();
        let mut ids = BTreeSet::new();
        let mut visited = BTreeSet::new();
        for _ in 0..500 {
            ensure!(
                visited.insert(url.as_str().to_owned()),
                "Repeated pagination token"
            );
            let payload = client
                .json(&url, &category_url(&unit.category_code))
                .await?;
            let data = &payload["data"];
            let items = data["list"]
                .as_array()
                .context("Missing or invalid data.list in listing response")?;
            let total = pagination_number(
                data,
                &[
                    "totalCount",
                    "total",
                    "count",
                    "totalElements",
                    "totalItems",
                    "goodsCount",
                ],
            );
            let before = products.len();
            for item in items {
                let product = listing_product(item, &unit)?;
                if ids.insert(product.product_id.clone()) {
                    products.push(product);
                }
                if max > 0 && products.len() >= max {
                    return Ok(());
                }
            }
            if total.is_some_and(|n| products.len() >= n) {
                return Ok(());
            }
            let next = data["pagination"]["nextPageUrl"]
                .as_str()
                .unwrap_or("")
                .trim();
            if next.is_empty() {
                ensure!(
                    !total.is_some_and(|n| products.len() < n),
                    "Pagination ended early: {} / {:?} products",
                    products.len(),
                    total
                );
                let has_next = data["pagination"]["hasNext"]
                    .as_bool()
                    .or_else(|| data["hasNext"].as_bool());
                ensure!(
                    has_next != Some(true),
                    "Next page exists but nextPageUrl is missing"
                );
                return Ok(());
            }
            ensure!(
                products.len() > before,
                "Next page contains no new products"
            );
            // Keep signed pagination tokens intact and never follow them to another origin.
            url = initial.join(next)?;
            ensure!(
                url.origin() == initial.origin() && url.path() == initial.path(),
                "Rejected pagination token pointing to a different API"
            );
        }
        bail!("Reached the pagination limit of 500 pages")
    }
    .await;
    ListingResult {
        unit,
        products,
        error: result.err().map(|e| format!("{e:#}")),
    }
}

pub fn parse_options(
    payload: &Value,
    product: &ProductRow,
) -> Result<(Vec<OptionRow>, Vec<SkuRow>)> {
    let items = payload["data"]["optionItems"]
        .as_array()
        .context("Missing or invalid data.optionItems in options response")?;
    let color = product_color(&product.product_name);
    let mut seen = BTreeSet::new();
    let mut options = Vec::new();
    let mut skus = Vec::new();
    for item in items {
        if item["isDeleted"].as_bool() == Some(true) {
            continue;
        }
        let mut size = scalar(&item["managedCode"]);
        if size.is_empty() {
            size = scalar(&item["optionValues"][0]["name"]);
        }
        ensure!(
            !size.is_empty(),
            "Missing option size for product {}",
            product.product_id
        );
        let activated = item["activated"]
            .as_bool()
            .context("Missing or invalid activated field in option")?;
        if !seen.insert((color.clone(), size.clone())) {
            continue;
        }
        let status = if activated { "판매중" } else { "품절" };
        let key = sku_key(&product.product_id, &color, &size);
        options.push(OptionRow {
            product_id: product.product_id.clone(),
            product_name: product.product_name.clone(),
            major_category: product.major_category.clone(),
            raw_category: product.raw_category.clone(),
            color: color.clone(),
            size: size.clone(),
            option_name: format!("{color} / {size}").trim_matches([' ', '/']).into(),
            option_status: status.into(),
            option_price: product
                .price
                .map(|p| p + number(&item["price"]).unwrap_or(0)),
            stock_qty: None,
            product_url: product.product_url.clone(),
        });
        skus.push(SkuRow {
            major_category: product.major_category.clone(),
            raw_category: product.raw_category.clone(),
            product_id: product.product_id.clone(),
            product_name: product.product_name.clone(),
            color: color.clone(),
            size,
            sku_key: key,
            sku_status: status.into(),
            product_url: product.product_url.clone(),
        });
    }
    Ok((options, skus))
}

pub async fn fetch_detail(
    client: &ApiClient,
    mut product: ProductRow,
    endpoint: &str,
) -> (ProductRow, Vec<OptionRow>, Vec<SkuRow>) {
    let result: Result<_> = async {
        let url = Url::parse(&format!("{endpoint}/{}/options", product.product_id))?;
        let payload = client.json(&url, &product.product_url).await?;
        parse_options(&payload, &product)
    }
    .await;
    match result {
        Ok((options, skus)) => {
            product.collection_status = if skus.is_empty() {
                CollectionStatus::Empty
            } else {
                CollectionStatus::Complete
            };
            (product, options, skus)
        }
        Err(error) => {
            product.collection_status = if error
                .downcast_ref::<RequestFailure>()
                .is_some_and(|e| matches!(e.status, Some(404 | 410)))
            {
                CollectionStatus::Unavailable
            } else {
                CollectionStatus::Failed
            };
            product.collection_error = Some(format!("{error:#}"));
            (product, vec![], vec![])
        }
    }
}
