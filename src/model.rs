use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Subcategory {
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Category {
    pub major_code: String,
    pub major_name: String,
    pub subs: Vec<Subcategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkUnit {
    pub major_code: String,
    pub major_category: String,
    pub category_code: String,
    pub raw_category: String,
}

impl WorkUnit {
    pub fn all_products() -> Self {
        Self {
            major_code: String::new(),
            major_category: "전체 상품".into(),
            category_code: String::new(),
            raw_category: "카테고리 미지정".into(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CollectionStatus {
    #[default]
    Pending,
    Complete,
    Empty,
    Unavailable,
    Failed,
    Skipped,
}

impl CollectionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "미수집",
            Self::Complete => "완료",
            Self::Empty => "옵션 없음",
            Self::Unavailable => "상품 없음/단종",
            Self::Failed => "수집 실패",
            Self::Skipped => "옵션 수집 생략",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductRow {
    pub crawl_date: String,
    pub gender_filter: String,
    pub brand: String,
    pub major_category: String,
    pub raw_category: String,
    pub category_code: String,
    pub product_id: String,
    pub product_name: String,
    pub product_url: String,
    pub price: Option<i64>,
    pub discount_rate: Option<String>,
    pub image_url: String,
    pub is_soldout: String,
    pub source_category_url: String,
    #[serde(default)]
    pub collection_status: CollectionStatus,
    #[serde(default)]
    pub collection_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionRow {
    pub product_id: String,
    pub product_name: String,
    pub major_category: String,
    pub raw_category: String,
    pub color: String,
    pub size: String,
    pub option_name: String,
    pub option_status: String,
    pub option_price: Option<i64>,
    pub stock_qty: Option<i64>,
    pub product_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkuRow {
    pub major_category: String,
    pub raw_category: String,
    pub product_id: String,
    pub product_name: String,
    pub color: String,
    pub size: String,
    pub sku_key: String,
    pub sku_status: String,
    pub product_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub stage: String,
    pub key: String,
    pub message: String,
}

pub fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn category_url(code: &str) -> String {
    if code.is_empty() {
        return format!("{}?gf=F", crate::config::BRAND_URL);
    }
    format!("{}?categoryCode={code}&gf=F", crate::config::BRAND_URL)
}

pub fn split_name_color(name: &str) -> (String, String) {
    static SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\[\]]+)\]\s*$").unwrap());
    static BRAND: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^무신사\s*스탠다드\s*\(MUSINSA\s*STANDARD\)\s*").unwrap()
    });
    static TAIL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*-\s*(?:사이즈\s*&?\s*)?후기.*$").unwrap());
    let name = clean(name);
    let (base, color) = match SUFFIX.captures(&name) {
        Some(c) => (&name[..c.get(0).unwrap().start()], clean(&c[1])),
        None => (name.as_str(), String::new()),
    };
    let base = BRAND.replace(base, "");
    (clean(&TAIL.replace(&base, "")), color)
}

pub fn product_color(name: &str) -> String {
    let (_, suffix) = split_name_color(name);
    if !suffix.is_empty() {
        return suffix;
    }
    let aliases = [
        ("스카이블루", "스카이블루"),
        ("라이트블루", "라이트블루"),
        ("아이보리", "IVORY"),
        ("화이트", "WHITE"),
        ("블랙", "BLACK"),
        ("네이비", "NAVY"),
        ("베이지", "BEIGE"),
        ("브라운", "BROWN"),
        ("그레이", "GREY"),
        ("그레이", "GRAY"),
        ("블루", "BLUE"),
        ("그린", "GREEN"),
        ("핑크", "PINK"),
        ("레드", "RED"),
        ("옐로우", "YELLOW"),
        ("퍼플", "PURPLE"),
        ("오렌지", "ORANGE"),
        ("크림", "CREAM"),
        ("오트밀", "OATMEAL"),
        ("민트", "MINT"),
        ("카키", "KHAKI"),
        ("차콜", "CHARCOAL"),
        ("멜란지", "멜란지"),
        ("데님", "DENIM"),
    ];
    let upper = name.to_uppercase();
    aliases
        .iter()
        .find(|(ko, en)| upper.contains(ko) || upper.contains(en))
        .map(|(ko, _)| (*ko).to_owned())
        .unwrap_or_default()
}

pub fn sku_key(id: &str, color: &str, size: &str) -> String {
    fn normalize(s: &str) -> String {
        let s = clean(s).replace('/', "-").replace(' ', "_");
        let s: String = s
            .chars()
            .filter(|c| {
                c.is_ascii_alphanumeric() || ('가'..='힣').contains(c) || *c == '_' || *c == '-'
            })
            .collect();
        if s.is_empty() { "UNKNOWN".into() } else { s }
    }
    format!("{}_{}_{}", normalize(id), normalize(color), normalize(size))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn suffix_preserves_other_brackets() {
        assert_eq!(
            split_name_color("무신사 스탠다드 (MUSINSA STANDARD) 우먼즈 [기획] [블랙]"),
            ("우먼즈 [기획]".into(), "블랙".into())
        );
        assert_eq!(product_color("라이트블루 셔츠"), "라이트블루");
        assert_eq!(
            sku_key("1001", "다크 그레이", "ONE SIZE"),
            "1001_다크_그레이_ONE_SIZE"
        );
    }
}
