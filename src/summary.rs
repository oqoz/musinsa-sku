use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::model::*;

/// Category choice is independent of asynchronous completion order.
/// A six-digit category wins over catch-all; ties use the smallest category code.
pub fn deduplicate_products(mut candidates: Vec<ProductRow>) -> Vec<ProductRow> {
    candidates.sort_by(|a, b| {
        b.category_code
            .len()
            .cmp(&a.category_code.len())
            .then(a.category_code.cmp(&b.category_code))
            .then(a.product_id.cmp(&b.product_id))
            .then(a.product_name.cmp(&b.product_name))
    });
    let mut by_id = BTreeMap::new();
    for product in candidates {
        by_id.entry(product.product_id.clone()).or_insert(product);
    }
    let mut products: Vec<_> = by_id.into_values().collect();
    products.sort_by(|a, b| {
        a.category_code
            .cmp(&b.category_code)
            .then(a.product_id.cmp(&b.product_id))
    });
    products
}

#[derive(Debug, Serialize)]
pub struct ProductSummary<'a> {
    pub major_category: &'a str,
    pub raw_category: &'a str,
    pub product_id: &'a str,
    pub product_name: &'a str,
    pub color_count: usize,
    pub size_count: usize,
    pub sku_count: usize,
    pub available_sku_count: usize,
    pub soldout_sku_count: usize,
    pub price: Option<i64>,
    pub product_url: &'a str,
    pub collection_status: CollectionStatus,
}

pub fn product_summaries<'a>(
    products: &'a [ProductRow],
    skus: &'a [SkuRow],
) -> Vec<ProductSummary<'a>> {
    let mut by_id: BTreeMap<&str, Vec<&SkuRow>> = BTreeMap::new();
    for sku in skus {
        by_id.entry(&sku.product_id).or_default().push(sku);
    }
    products
        .iter()
        .map(|p| {
            let rows = by_id
                .get(p.product_id.as_str())
                .cloned()
                .unwrap_or_default();
            let mut distinct = BTreeMap::new();
            for s in rows {
                distinct.entry((&s.color, &s.size)).or_insert(s);
            }
            ProductSummary {
                major_category: &p.major_category,
                raw_category: &p.raw_category,
                product_id: &p.product_id,
                product_name: &p.product_name,
                color_count: distinct
                    .values()
                    .filter(|s| !s.color.is_empty())
                    .map(|s| &s.color)
                    .collect::<BTreeSet<_>>()
                    .len(),
                size_count: distinct
                    .values()
                    .map(|s| &s.size)
                    .collect::<BTreeSet<_>>()
                    .len(),
                sku_count: distinct.len(),
                available_sku_count: distinct
                    .values()
                    .filter(|s| s.sku_status == "판매중")
                    .count(),
                soldout_sku_count: distinct.values().filter(|s| s.sku_status == "품절").count(),
                price: p.price,
                product_url: &p.product_url,
                collection_status: p.collection_status,
            }
        })
        .collect()
}

#[derive(Debug)]
pub struct ModelGroup {
    pub major: String,
    pub category: String,
    pub name: String,
    pub product_ids: BTreeSet<String>,
    pub colors: BTreeSet<String>,
    pub sizes: BTreeSet<String>,
    /// Product IDs remain part of SKU identity, even when display names coincide.
    pub sku_keys: BTreeSet<(String, String, String)>,
    pub statuses: BTreeMap<CollectionStatus, usize>,
    pub prices: Vec<i64>,
}

impl ModelGroup {
    pub fn status_label(&self) -> String {
        self.statuses
            .iter()
            .map(|(status, count)| format!("{} {count}개", status.label()))
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

pub fn model_groups(products: &[ProductRow], skus: &[SkuRow]) -> Vec<ModelGroup> {
    let mut groups: BTreeMap<(String, String, String), ModelGroup> = BTreeMap::new();
    let mut product_groups = BTreeMap::new();
    for product in products {
        let (name, _) = split_name_color(&product.product_name);
        let key = (
            product.major_category.clone(),
            product.raw_category.clone(),
            name.clone(),
        );
        product_groups.insert(product.product_id.as_str(), key.clone());
        let group = groups.entry(key).or_insert_with(|| ModelGroup {
            major: product.major_category.clone(),
            category: product.raw_category.clone(),
            name,
            product_ids: BTreeSet::new(),
            colors: BTreeSet::new(),
            sizes: BTreeSet::new(),
            sku_keys: BTreeSet::new(),
            statuses: BTreeMap::new(),
            prices: Vec::new(),
        });
        // Keep failed/empty/skipped products visible; never turn missing data into a silent omission.
        group.product_ids.insert(product.product_id.clone());
        group.colors.insert(product_color(&product.product_name));
        *group.statuses.entry(product.collection_status).or_default() += 1;
        if let Some(price) = product.price {
            group.prices.push(price);
        }
    }
    for sku in skus {
        if let Some(key) = product_groups.get(sku.product_id.as_str()) {
            let group = groups.get_mut(key).unwrap();
            group.colors.insert(sku.color.clone());
            group.sizes.insert(sku.size.clone());
            group
                .sku_keys
                .insert((sku.product_id.clone(), sku.color.clone(), sku.size.clone()));
        }
    }
    groups.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{listing_product, parse_options};
    use serde_json::json;

    pub fn product(id: &str, color: &str, code: &str) -> ProductRow {
        listing_product(
            &json!({"goodsNo":id,"goodsName":format!("티셔츠 [{color}]"),"normalPrice":19900}),
            &WorkUnit {
                major_code: "001".into(),
                major_category: "상의".into(),
                category_code: code.into(),
                raw_category: if code.len() == 6 {
                    "반소매"
                } else {
                    "상의(미분류)"
                }
                .into(),
            },
        )
        .unwrap()
    }

    #[test]
    fn category_assignment_is_order_independent() {
        let a = product("1", "화이트", "001");
        let b = product("1", "화이트", "001002");
        let c = product("1", "화이트", "001001");
        let first = deduplicate_products(vec![a.clone(), b.clone(), c.clone()]);
        let second = deduplicate_products(vec![c, b, a]);
        assert_eq!(first[0].category_code, "001001");
        assert_eq!(first[0].category_code, second[0].category_code);
    }

    #[test]
    fn counts_existing_skus_instead_of_a_cartesian_product() {
        let mut white = product("1", "화이트", "001001");
        let mut black = product("2", "블랙", "001001");
        white.collection_status = CollectionStatus::Complete;
        black.collection_status = CollectionStatus::Complete;
        let (_, mut skus) = parse_options(&json!({"data":{"optionItems":[{"managedCode":"S","activated":true},{"managedCode":"M","activated":false}]}}), &white).unwrap();
        skus.extend(
            parse_options(
                &json!({"data":{"optionItems":[{"managedCode":"M","activated":true}]}}),
                &black,
            )
            .unwrap()
            .1,
        );
        let groups = model_groups(&[white, black], &skus);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].colors.len(), 2);
        assert_eq!(groups[0].sizes.len(), 2);
        assert_eq!(groups[0].sku_keys.len(), 3);
    }

    #[test]
    fn failed_products_remain_visible_with_a_status() {
        let mut p = product("1", "블랙", "001001");
        p.collection_status = CollectionStatus::Failed;
        let groups = model_groups(&[p], &[]);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].status_label().contains("수집 실패"));
        assert_eq!(groups[0].sku_keys.len(), 0);
    }
}
