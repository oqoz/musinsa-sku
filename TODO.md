# TODO

## Rust 재작성

- [x] 실행 입력을 출력 폴더와 요청별 timeout으로 축소. fast 기본값·전체 상품·모든 옵션·CSV/BOM 고정, Rust YAML 설정 제거.
- [x] 기본 출력을 CSV 9개 + 실행 기록·스키마 manifest로 전환. 모델별 합계와 분류 합계를 분리.
- [x] 빈 테이블 헤더, UTF-8/BOM, 한글·쉼표·줄바꿈, 문자열 ID, null, 실패 상품 보존 검증.
- [x] 이전 결과 묶음 교체 시 사용자 추가 파일 보존. XLSX writer는 개발 비교용으로 유지.

- [x] Python 없이 실행하는 API 수집·카테고리 탐색·Excel 생성 파이프라인.
- [x] 호스트별 요청 간격, 공통 429 대기·재시도, 수집 상태·오류 기록.
- [x] 6자리 분류 우선 및 코드 순서로 결정하는 상품 카테고리 배정.
- [x] 실제 상품 ID·색상·사이즈 조합으로 SKU 집계. 실패 상품도 요약에 표시.
- [x] 카테고리 캐시, 실행 snapshot, 네트워크 없는 엑셀 재생성.

아래 데이터 신뢰도 항목은 Rust 버전에 적용했다. 비교용으로 보존한 Python 구현에는 기존 문제가 남아 있다.
Phase B/C 역시 기존 Python 구현의 후속 계획이다.

## 데이터 신뢰도 — 성능 측정과 후속 작업에서 유지할 확인 사항

- [ ] 옵션 API의 403/429 등 수집 실패를 실제 옵션 없음과 구분한다. SKU 0건인 상품이 요약에서 빠지므로, 수집 실패로 인한 누락을 성공으로 해석하지 않는다.
- [ ] 상품 중복 제거 시 비동기 응답 도착 순서로 카테고리가 결정되지 않게 한다. 세부 분류와 대분류 catch-all 사이의 선택 규칙을 고정한다.
- [ ] `모델 SKU 개수`의 색상 수 × 전체 사이즈 종류 수와 실제 존재하는 색상·사이즈 조합 수를 구분한다. 색상별 사이즈 구성이 다른 경우를 검증한다.

## Phase B: Selenium 의존성 정리 (Phase A 안정화 후)

옵션 API 전환으로 detail 경로가 Selenium 비의존이 됐다. 남은 `pool.borrow` 사이트는 두 곳뿐.

- [ ] `discover_category_tree` ([categories.py](src/musinsa/categories.py)) 도 가능하면 HTTP 직접 호출로 (현재는 `driver.page_source` 의존). 카테고리 인덱스 페이지가 SSR 인지 SPA 인지 확인 필요. SSR 이면 `aiohttp` GET 한 번으로 끝.
- [ ] [listing.py](src/musinsa/listing.py) 의 Selenium 폴백 (`scroll_and_collect_products`, `wait_for_product_items`, `parse_product_items_from_html`) 제거 검토. PLP API 가 안정적이면 (직전 6개월 운영에서 한 번도 폴백이 트리거되지 않으면) 제거 OK.
- [ ] [detail.py](src/musinsa/detail.py) 의 Selenium 폴백 (`collect_detail_and_options` 외) 도 같은 기준으로 제거 검토.
- [ ] 위 세 사이트 모두 제거되면 [browser.py](src/musinsa/browser.py), [driver_pool.py](src/musinsa/driver_pool.py) 삭제, [pyproject.toml](pyproject.toml) 의 `selenium` 의존성 제거. README/CLAUDE.md 의 "Firefox/Chrome" 관련 설명 정리.

## Phase C: tags / stat 보강 (선택)

옵션 수집과 같은 `aiohttp` gather 안에 추가 호출 가능. 한 상품당 호출 3개로 늘어도 동시성 32 면 시간 영향 미미.

- [ ] [schemas.py](src/musinsa/schemas.py) `ProductRow` 에 `tags: str` (콤마 join), `purchase_total: Optional[int]`, `view_total: Optional[int]` 추가
- [ ] [detail_api.py](src/musinsa/detail_api.py) 에 `fetch_goods_tags_payload`, `fetch_goods_stat_payload` 추가 (확정된 엔드포인트: `goods-detail.musinsa.com/api2/goods/{pid}/tags`, `.../stat`)
- [ ] `gather_details_async` 에서 옵션 + tags + stat 을 `asyncio.gather` 로 동시 호출, 모이면 ProductRow 갱신
- [ ] [excel.py](src/musinsa/excel.py) products_raw 컬럼은 `__annotations__` 기반이라 자동 반영. `SKU_요약` 시트 양식은 건드리지 않음.

## 의도적으로 안 건드리는 것

- `TARGET_CATEGORIES`, `GENDER_FILTER="F"`, `CATEGORY_MASTER`, `COLOR_ALIASES`, 사이즈/가격 정규식 — CLAUDE.md에 명시된 데모 스코프.
- 엑셀 시트 순서 및 `SKU_요약` 병합 규칙.
- dataclass 스키마 (`ProductRow`/`OptionRow`/`SkuRow`) — Phase C 확장 시에만 필드 추가 (기존 필드 보존).
