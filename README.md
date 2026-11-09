# Musinsa Standard Woman SKU

Get SKUs of [Musinsa Standard Woman](https://www.musinsa.com/brand/musinsastandardwoman) products via the web API.

## Execution

Run the following command with a [stable version of Rust](https://releases.rs/) installed.

- `output` (optional): location of output `.csv` files (default: `outputs/musinsa_all`).
- `timeout` (optional): timeout per request in seconds (default: `20`).

```bash
cargo run -- --output outputs/musinsa_all --timeout 20
```

| Item | Fixed behavior |
| --- | --- |
| Scope | All Musinsa Standard Woman products |
| Request interval | At least 100ms per host, about 10 requests/s |
| Option concurrency | 8 |
| Retries | Up to 5 |
| Output | 9 CSV files |
| Encoding | UTF-8 BOM, CRLF |

## Output directory

| File | Contents |
| --- | --- |
| `sku_summary.csv` | One row per model group: product IDs, colors, sizes, actual SKU count, prices and collection status |
| `category_summary.csv` | One row per category pair: known SKU count, share, prices and failed product count |
| `model_summary.csv` | One row per product ID: color, size, available and sold-out SKU counts |
| `products_raw.csv` | All products with collection status and errors, including failed products and products without options |
| `options_raw.csv` | Actual option combinations, status and prices |
| `sku_result.csv` | One row per actual product ID, color and size combination |
| `category_master.csv` | Category list; headers only in the current full collection |
| `collection_issues.csv` | Unresolved errors; headers remain when there are no errors |
| `run_summary.csv` | Collection scope, completion status, counts and stage timings |
| `run.json` | Full collection data, HTTP requests and run records |
| `manifest.json` | File row counts, sizes, column order, types, nullability, encoding and time zones |

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
