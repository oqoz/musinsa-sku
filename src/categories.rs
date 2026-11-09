use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use reqwest::Url;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use thirtyfour::{
    common::capabilities::desiredcapabilities::PageLoadStrategy, manager::BrowserKind, prelude::*,
};

use crate::{
    api::ApiClient,
    config::{BRAND, BRAND_URL, Config, USER_AGENT},
    model::*,
};

type Pair = (String, String, String);

pub fn parse_pairs(html: &str) -> Vec<Pair> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("[data-category-id]").unwrap();
    let mut seen = BTreeSet::new();
    document
        .select(&selector)
        .filter_map(|element| {
            let id = element.value().attr("data-category-id")?;
            if !matches!(id.len(), 3 | 6) || !id.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let pair = (
                id.into(),
                clean(element.value().attr("data-category-name").unwrap_or("")),
                clean(element.value().attr("data-section-name").unwrap_or("")),
            );
            seen.insert(pair.clone()).then_some(pair)
        })
        .collect()
}

pub fn category_from_pairs(code: &str, pairs: &[Pair]) -> Option<Category> {
    let mut major_name = String::new();
    let mut subs = BTreeMap::new();
    for (id, full, _) in pairs {
        if !id.starts_with(code) {
            continue;
        }
        let parts: Vec<_> = full
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "(not set)")
            .collect();
        if parts.len() >= 2 {
            major_name = parts[0].into();
        }
        if id.len() == 6 && !parts.is_empty() {
            subs.entry(id.clone())
                .or_insert_with(|| parts[parts.len().min(2) - 1].to_owned());
        }
    }
    if subs.is_empty() {
        return None;
    }
    if major_name.is_empty() {
        major_name = major_fallback(code).into();
    }
    Some(Category {
        major_code: code.into(),
        major_name,
        subs: subs
            .into_iter()
            .map(|(code, name)| Subcategory { code, name })
            .collect(),
    })
}

fn major_fallback(code: &str) -> &str {
    match code {
        "001" => "상의",
        "002" => "아우터",
        "003" => "바지",
        "004" | "104" => "가방",
        "026" => "속옷/홈웨어",
        "100" => "원피스/스커트",
        "101" => "소품",
        "102" | "120" => "모자",
        "103" => "신발",
        "017" | "005" => "스포츠/레저",
        "020" => "스커트",
        "022" => "원피스",
        _ => code,
    }
}

#[derive(Serialize, Deserialize)]
struct Cache {
    version: u32,
    brand: String,
    gender: String,
    fetched_at: DateTime<Utc>,
    tree: Vec<Category>,
}

pub fn validate_tree(tree: &[Category]) -> Result<()> {
    ensure!(!tree.is_empty(), "Category tree is empty");
    let mut codes = BTreeSet::new();
    for category in tree {
        ensure!(
            category.major_code.len() == 3
                && category.major_code.bytes().all(|b| b.is_ascii_digit())
                && !category.major_name.is_empty(),
            "Invalid major category"
        );
        ensure!(
            codes.insert(category.major_code.clone()),
            "Duplicate major category"
        );
        ensure!(
            !category.subs.is_empty(),
            "No subcategories for category {}",
            category.major_code
        );
        for sub in &category.subs {
            ensure!(
                sub.code.len() == 6
                    && sub.code.starts_with(&category.major_code)
                    && sub.code.bytes().all(|b| b.is_ascii_digit())
                    && !sub.name.is_empty(),
                "Invalid subcategory: {}",
                sub.code
            );
            ensure!(
                codes.insert(sub.code.clone()),
                "Duplicate subcategory: {}",
                sub.code
            );
        }
    }
    Ok(())
}

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut data = serde_json::to_vec_pretty(value)?;
    data.push(b'\n');
    std::fs::write(&temp, data)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

async fn browser(config: &Config) -> Result<WebDriver> {
    let chrome = config.browser == "chrome" || (config.browser == "auto" && !in_path("firefox"));
    let (caps, kind): (Capabilities, _) = if chrome {
        let mut caps = DesiredCapabilities::chrome();
        if config.headless {
            caps.add_arg("--headless=new")?;
        }
        caps.add_arg("--no-sandbox")?;
        caps.add_arg("--disable-dev-shm-usage")?;
        caps.add_arg(&format!("--user-agent={USER_AGENT}"))?;
        caps.set_page_load_strategy(PageLoadStrategy::Eager)?;
        (caps.into(), BrowserKind::Chrome)
    } else {
        let mut caps = DesiredCapabilities::firefox();
        if config.headless {
            caps.set_headless()?;
        }
        caps.set_browser_option("prefs", serde_json::json!({"intl.accept_languages":"ko-KR,ko,en-US,en", "general.useragent.override": USER_AGENT}))?;
        caps.set_page_load_strategy(PageLoadStrategy::Eager)?;
        (caps.into(), BrowserKind::Firefox)
    };
    let mut builder = WebDriver::managed(caps)
        .cache_dir(PathBuf::from(".cache/musinsa/webdriver"))
        .download_timeout(Duration::from_secs(60));
    let env_name = if chrome {
        "MUSINSA_CHROMEDRIVER"
    } else {
        "MUSINSA_GECKODRIVER"
    };
    if let Some(path) = std::env::var_os(env_name) {
        builder = builder.driver_binary(kind, PathBuf::from(path));
    }
    let driver = builder
        .await
        .context("Failed to start browser. Check your Firefox or Chrome installation")?;
    if let Err(error) = driver
        .set_page_load_timeout(Duration::from_secs(config.timeout_seconds))
        .await
    {
        let _ = driver.quit().await;
        return Err(error.into());
    }
    Ok(driver)
}

fn in_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p)
            .any(|d| d.join(name).is_file() || d.join(format!("{name}.exe")).is_file())
    })
}

async fn browser_pairs(driver: &WebDriver, url: &str, code: Option<&str>) -> Result<Vec<Pair>> {
    driver.goto(url).await?;
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut previous = Vec::new();
    let mut stable = 0;
    loop {
        let pairs = parse_pairs(&driver.source().await?);
        let ready = pairs.iter().any(|(id, _, section)| match code {
            Some(code) => id.len() == 6 && id.starts_with(code),
            None => id.len() == 3 && section == "menu_tab",
        });
        if ready && pairs == previous {
            stable += 1;
        } else {
            stable = 0;
        }
        if stable >= 2 {
            return Ok(pairs);
        }
        if Instant::now() >= deadline {
            ensure!(ready, "Timed out waiting for category DOM: {url}");
            return Ok(pairs);
        }
        previous = pairs;
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

async fn get_pairs(
    client: &ApiClient,
    driver: &mut Option<WebDriver>,
    config: &Config,
    url: &str,
    code: Option<&str>,
) -> Result<Vec<Pair>> {
    if driver.is_none() {
        if let Ok(html) = client.text(&Url::parse(url)?, BRAND_URL).await {
            let pairs = parse_pairs(&html);
            let ready = pairs.iter().any(|(id, _, section)| match code {
                Some(code) => id.len() == 6 && id.starts_with(code),
                None => id.len() == 3 && section == "menu_tab",
            });
            if ready {
                return Ok(pairs);
            }
        }
        println!("Categories unavailable over HTTP; starting a browser");
        *driver = Some(browser(config).await?);
    }
    browser_pairs(driver.as_ref().unwrap(), url, code).await
}

pub async fn discover(
    client: &ApiClient,
    config: &Config,
    refresh: bool,
) -> Result<(Vec<Category>, Vec<Issue>)> {
    if !refresh
        && let Ok(bytes) = std::fs::read(&config.category_cache)
        && let Ok(cache) = serde_json::from_slice::<Cache>(&bytes)
        && cache.version == 1
        && cache.brand == BRAND
        && cache.gender == "F"
        && (Utc::now() - cache.fetched_at).num_seconds() >= 0
        && (Utc::now() - cache.fetched_at).num_seconds()
            < config
                .category_cache_hours
                .saturating_mul(3600)
                .min(i64::MAX as u64) as i64
        && validate_tree(&cache.tree).is_ok()
    {
        println!("Using category cache: {}", config.category_cache.display());
        return Ok((cache.tree, vec![]));
    }
    let mut driver = None;
    let result: Result<_> = async {
        let seed = get_pairs(client, &mut driver, config, &category_url("001"), None).await?;
        let codes: BTreeSet<_> = seed
            .iter()
            .filter(|(id, _, section)| id.len() == 3 && section == "menu_tab")
            .map(|p| p.0.clone())
            .collect();
        ensure!(!codes.is_empty(), "Brand category menu was not found");
        let mut tree = Vec::new();
        let mut issues = Vec::new();
        for code in codes {
            let url = format!("https://www.musinsa.com/category/{code}?gf=F");
            let result = get_pairs(client, &mut driver, config, &url, Some(&code))
                .await
                .and_then(|pairs| {
                    category_from_pairs(&code, &pairs).context("No subcategories found")
                });
            match result {
                Ok(category) => {
                    println!(
                        "  {} {}: {} subcategories",
                        code,
                        category.major_name,
                        category.subs.len()
                    );
                    tree.push(category);
                }
                Err(error) => issues.push(Issue {
                    stage: "categories".into(),
                    key: code,
                    message: format!("{error:#}"),
                }),
            }
        }
        validate_tree(&tree)?;
        if issues.is_empty() {
            write_json(
                &config.category_cache,
                &Cache {
                    version: 1,
                    brand: BRAND.into(),
                    gender: "F".into(),
                    fetched_at: Utc::now(),
                    tree: tree.clone(),
                },
            )?;
        }
        Ok((tree, issues))
    }
    .await;
    if let Some(driver) = driver
        && let Err(error) = driver.quit().await
    {
        eprintln!("Failed to close browser: {error}");
    }
    result
}

pub fn work_units(tree: &[Category], selected: &[String]) -> Result<Vec<WorkUnit>> {
    let mut units = Vec::new();
    for c in tree {
        for sub in &c.subs {
            units.push(WorkUnit {
                major_code: c.major_code.clone(),
                major_category: c.major_name.clone(),
                category_code: sub.code.clone(),
                raw_category: sub.name.clone(),
            });
        }
        units.push(WorkUnit {
            major_code: c.major_code.clone(),
            major_category: c.major_name.clone(),
            category_code: c.major_code.clone(),
            raw_category: format!("{}(미분류)", c.major_name),
        });
    }
    for code in selected {
        ensure!(
            units.iter().any(|u| u.category_code == *code),
            "Category code {code} was not found in the category tree"
        );
    }
    units.retain(|u| {
        selected.is_empty()
            || selected
                .iter()
                .any(|c| u.category_code == *c || (c.len() == 3 && u.major_code == *c))
    });
    units.sort_by(|a, b| a.category_code.cmp(&b.category_code));
    Ok(units)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_and_scopes_category_tree() {
        let html = r#"<a data-category-id="001" data-section-name="menu_tab"></a><a data-category-id="001001" data-category-name="상의 | 반소매 티셔츠"></a><a data-category-id="002001" data-category-name="아우터 | 코트"></a>"#;
        let c = category_from_pairs("001", &parse_pairs(html)).unwrap();
        assert_eq!(c.subs.len(), 1);
        assert_eq!(c.major_name, "상의");
        assert_eq!(
            work_units(std::slice::from_ref(&c), &["001001".into()])
                .unwrap()
                .len(),
            1
        );
        assert!(work_units(&[c], &["999999".into()]).is_err());
    }
}
