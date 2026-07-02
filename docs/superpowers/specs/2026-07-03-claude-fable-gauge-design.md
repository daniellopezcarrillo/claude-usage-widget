# Claude Fable 전용 소모 게이지 표시 — 설계

날짜: 2026-07-03

## 배경

claude.ai 요금제에 Fable 모델 전용 주간 소모 한도가 생겼다. OAuth usage API
(`GET https://api.anthropic.com/api/oauth/usage`) 응답을 실제로 확인한 결과,
이 게이지는 기존 최상위 키(`seven_day_opus` 등)가 아니라 새로운 `limits` 배열로 내려온다:

```json
"limits": [
  {"kind":"session",       "group":"session", "percent":25, "resets_at":"...", "is_active":false},
  {"kind":"weekly_all",    "group":"weekly",  "percent":20, "resets_at":"...", "is_active":false},
  {"kind":"weekly_scoped", "group":"weekly",  "percent":31, "resets_at":"...",
   "scope":{"model":{"id":null,"display_name":"Fable"},"surface":null}, "is_active":true}
]
```

현재 앱(`src-tauri/src/providers/claude.rs`)은 최상위 키만 파싱하므로 Fable 게이지(31%)를 놓친다.
기존 최상위 모델별 키는 이 계정 응답에서 전부 `null`이었다.

## 결정: `limits` 배열을 1차 소스로 전환

프론트엔드(`ProviderCard`, `MiniGauge`)는 `windows` 배열을 그대로 렌더링하므로
**Rust 파싱 계층(`claude.rs`)만 변경**한다.

### 파싱 구조

`RawUsage`에 `limits: Option<Vec<RawLimit>>` 추가.

- `RawLimit`: `kind`(String), `group`(String), `percent`(f64), `resets_at`(Option\<String\>), `scope`(Option)
- `scope.model.display_name`(Option\<String\>)으로 모델명 취득
- 그 외 필드(`severity`, `is_active` 등)는 무시

### 매핑 규칙

`limits`가 존재하고 매핑 결과 윈도우가 1개 이상이면 이를 사용:

| 조건 | key | 라벨 | 기간(time_progress) |
|---|---|---|---|
| `group == "session"` | `five_hour` | `5시간` | 5시간 |
| `kind == "weekly_all"` | `seven_day` | `7일` | 7일 |
| `kind == "weekly_scoped"` + 모델명 존재 | `weekly_scoped_<모델명 소문자>` | `7일 (<display_name>)` | 7일 |
| 그 외 (알 수 없는 group, 모델명 없는 scoped) | 건너뜀 | | |

- `five_hour`/`seven_day` key 재사용으로 기존 동작(상태 저장, 미니 뷰 축약 `5h`/`7d`)과 호환.
- 모델명이 동적이므로 향후 다른 모델 전용 게이지도 코드 수정 없이 표시된다.
- `MiniGauge.shortLabel`은 `7일 (Fable)` → `7d(f)`로 기존 정규식이 그대로 처리한다.

### 폴백

`limits`가 없거나(`null`/부재) 매핑 결과가 비면 기존 `WIN_DEFS` 최상위 키 파싱을 그대로 사용한다.

### 에러 처리

- `resets_at`이 없거나 빈 문자열 → 기존과 동일하게 `resets_at: ""`, `time_progress: 100`.
- 응답 전체 파싱 실패 시 기존 경로(`AppError::Expired`) 유지.

## 테스트

`claude.rs` 단위 테스트 추가:

1. 실제 응답 형태의 JSON에서 `limits` 3건 → 윈도우 3개, Fable 라벨/키 확인
2. `limits: null` → 기존 최상위 키 폴백
3. 모델명 없는 `weekly_scoped` → 건너뜀
