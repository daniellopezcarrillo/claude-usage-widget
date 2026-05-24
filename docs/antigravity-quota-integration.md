# Antigravity (agy) Quota 추적 통합 가이드

claude_usage 위젯이 Antigravity CLI(`agy`)의 사용량을 어떻게 읽어오는지, 다른 서비스에 같은 기능을 옮길 때 필요한 모든 사실을 정리한 문서. 2026-05-25 시점 실측 + 코드 통합 결과 기준.

---

## 1. 토큰 저장 위치 (Windows)

agy는 OAuth 토큰을 **Windows Credential Manager**에 Generic credential로 저장한다. 기존 `~/.gemini/oauth_creds.json`도 함께 갱신되지만 그건 legacy gemini-cli 호환용일 뿐 agy 본인 토큰은 wincred에만 있다.

| 항목 | 값 |
|---|---|
| Storage | Windows wincred (Generic credential) |
| Target name | `gemini:antigravity` |
| Type | `CRED_TYPE_GENERIC` (1) |
| Persist | Local-machine |
| Library used by agy | `zalando/go-keyring` (형식: `service:username`) |

### Blob 포맷

UTF-8 JSON, 약 500 바이트:

```json
{
  "auth_method": "consumer",
  "token": {
    "access_token": "ya29.a0AQv...",
    "token_type": "Bearer",
    "refresh_token": "1//0e58...",
    "expiry": "2026-05-25T07:47:41.1314927+09:00"
  }
}
```

- `access_token`: ~260자, Bearer 토큰. 1시간 TTL.
- `refresh_token`: ~95자.
- `expiry`: RFC 3339, 보통 1시간 후.

### 추출 방법 (언어별)

**Python (ctypes)**:
```python
import ctypes, json
from ctypes import wintypes as wt

adv = ctypes.WinDLL("Advapi32.dll", use_last_error=True)

class CREDENTIALW(ctypes.Structure):
    _fields_ = [
        ("Flags", wt.DWORD), ("Type", wt.DWORD),
        ("TargetName", wt.LPWSTR), ("Comment", wt.LPWSTR),
        ("LastWritten", ctypes.c_uint64),
        ("CredentialBlobSize", wt.DWORD),
        ("CredentialBlob", ctypes.POINTER(ctypes.c_ubyte)),
        ("Persist", wt.DWORD), ("AttributeCount", wt.DWORD),
        ("Attributes", ctypes.c_void_p),
        ("TargetAlias", wt.LPWSTR), ("UserName", wt.LPWSTR),
    ]

PCRED = ctypes.POINTER(CREDENTIALW)
adv.CredReadW.argtypes = [wt.LPCWSTR, wt.DWORD, wt.DWORD, ctypes.POINTER(PCRED)]
adv.CredReadW.restype = wt.BOOL
adv.CredFree.argtypes = [ctypes.c_void_p]

pc = PCRED()
if adv.CredReadW("gemini:antigravity", 1, 0, ctypes.byref(pc)):
    c = pc.contents
    blob = ctypes.string_at(c.CredentialBlob, c.CredentialBlobSize)
    data = json.loads(blob.decode("utf-8").rstrip("\x00"))
    access_token = data["token"]["access_token"]
    adv.CredFree(pc)
```

**Rust (windows-sys)**:
```rust
use windows_sys::Win32::Security::Credentials::{
    CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
};

pub fn read_token_blob(target: &str) -> Option<Vec<u8>> {
    let target_w: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut p_cred: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        if CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p_cred) == 0 {
            return None;
        }
        let c = &*p_cred;
        let blob = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec();
        CredFree(p_cred as *mut _);
        Some(blob)
    }
}
```

Cargo.toml:
```toml
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = ["Win32_Security_Credentials", "Win32_Foundation"] }
```

### ⚠ PowerShell P/Invoke 함정

`CredEnumerate(NULL, 0, ...)`를 PowerShell에서 호출하면 marshaling 버그로 0개 반환된다. Python/Rust/C에서는 정상이다. 진단할 때 PowerShell을 쓰지 마라.

---

## 2. OAuth Client / Scope

`oauth_creds.json` 토큰 (cloud-platform/email/profile)으로는 `fetchAvailableModels`가 **403 PERMISSION_DENIED**. agy 본인 토큰만 통과. 차이는 scope.

### agy 본인이 쓰는 OAuth Client

`agy.exe`(보통 `%LOCALAPPDATA%\agy\bin\agy.exe`) strings에서 추출 가능. 공개 desktop OAuth client (PKCE/installed app)라 사용자 binary에 노출돼있다. 직접 추출 방법:

```bash
# client_id 후보
grep -oE '[0-9]{10,13}-[a-z0-9]{20,40}\.apps\.googleusercontent\.com' agy.exe | sort -u
# client_secret 후보 (Google desktop client는 GOCSPX- prefix)
grep -oE 'GOCSPX-[A-Za-z0-9_-]{20,40}' agy.exe | sort -u
```

token endpoint는 `https://oauth2.googleapis.com/token` 고정.

추출 후 자체 OAuth flow에 사용. (이 문서에서는 push protection 우회를 위해 실제 값 생략 — 공개된 3rd-party 레포 [skainguyen1412/antigravity-usage](https://github.com/skainguyen1412/antigravity-usage)에도 동일 값 노출됨.)

### Scope 차이

| 기존 gemini-cli (`oauth_creds.json`) | agy 본인 토큰 |
|---|---|
| `cloud-platform` | `cloud-platform` |
| `userinfo.email` | `userinfo.email` |
| `userinfo.profile` | `userinfo.profile` |
| `openid` | - |
| - | **`cclog`** |
| - | **`experimentsandconfigs`** |

`cclog`와 `experimentsandconfigs`가 fetchAvailableModels 인가 핵심. 새 OAuth flow를 자체 구현하려면 이 scope 둘을 포함해야 한다.

### Refresh

```bash
curl -X POST https://oauth2.googleapis.com/token \
  -d "client_id=$AGY_CLIENT_ID" \
  -d "client_secret=$AGY_CLIENT_SECRET" \
  -d "refresh_token=$REFRESH" \
  -d "grant_type=refresh_token"
```

Google OAuth는 refresh_token rotation을 하지 않으므로 agy와 race 없이 refresh 가능.

---

## 3. API Endpoints

### 3.1 `loadCodeAssist` — 프로젝트 ID 얻기

```http
POST https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist
Authorization: Bearer <access_token>
Content-Type: application/json
User-Agent: antigravity

{"metadata":{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}}
```

**응답 (200)**:
```json
{
  "currentTier": {"id": "standard-tier", "name": "Gemini Code Assist", ...},
  "allowedTiers": [...],
  "paidTier": {"id": "g1-pro-tier", "name": "Gemini Code Assist in Google One AI Pro"},
  "cloudaicompanionProject": "ivory-life-l98r4",
  "gcpManaged": false,
  "upgradeSubscriptionUri": "..."
}
```

여기서 `cloudaicompanionProject`를 추출. 응답에 따라 string 또는 `{id, name}` 객체일 수 있어 둘 다 다뤄야 함.

### 3.2 `fetchAvailableModels` — 모델별 quota

```http
POST https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels
Authorization: Bearer <access_token>
Content-Type: application/json
User-Agent: antigravity

{"project":"ivory-life-l98r4"}
```

**응답 (200)** — 약 19 entries (Gemini 11, Claude 2, GPT-OSS 1, internal 4):

```json
{
  "models": {
    "gemini-3.1-pro-high": {
      "displayName": "Gemini 3.1 Pro (High)",
      "supportsImages": true,
      "supportsThinking": true,
      "thinkingBudget": 10001,
      "recommended": true,
      "maxTokens": 1048576,
      "maxOutputTokens": 65535,
      "tokenizerType": "LLAMA_WITH_SPECIAL",
      "quotaInfo": {
        "remainingFraction": 1,
        "resetTime": "2026-05-24T21:48:36Z"
      },
      "model": "MODEL_PLACEHOLDER_M37",
      "apiProvider": "API_PROVIDER_GOOGLE_GEMINI",
      "modelProvider": "MODEL_PROVIDER_GOOGLE",
      "tagTitle": "New"
    },
    "claude-sonnet-4-6": {
      "displayName": "Claude Sonnet 4.6 (Thinking)",
      "quotaInfo": { "remainingFraction": 1, "resetTime": "2026-05-24T23:38:57Z" },
      ...
    },
    "gpt-oss-120b-medium": { ... },
    "chat_23310": {
      "isInternal": true,
      "quotaInfo": { "remainingFraction": 1 }
    }
  },
  "agentModelSorts": [...],
  "defaultAgentModelId": "...",
  "deprecatedModelIds": [...],
  "experimentIds": [...],
  "tabModelIds": [...],
  "commandModelIds": [...],
  "imageGenerationModelIds": [...],
  "audioTranscriptionModelIds": [...],
  "mqueryModelIds": [...],
  "webSearchModelIds": [...],
  "commitMessageModelIds": [...],
  "tieredModelIds": [...]
}
```

### 핵심 필드

| 필드 | 의미 |
|---|---|
| `models[id].displayName` | 사용자 표시명 (`Gemini 3.5 Flash (Medium)` 등). 없으면 internal 모델 — skip 대상 |
| `models[id].quotaInfo.remainingFraction` | 잔여 비율 0.0~1.0. utilization = `(1 - remainingFraction) * 100` |
| `models[id].quotaInfo.resetTime` | RFC 3339, 다음 리셋 시각. Gemini는 ~5h 후, Claude/GPT-OSS는 다른 cycle |
| `models[id].recommended` | UI 강조 후보 |
| `models[id].isInternal` | true면 사용자에게 노출 안 하는 것이 통상 |

### 모델 ID 명명 규칙

- Gemini: `gemini-<version>-<family>[-<effort>]` (예: `gemini-3.1-pro-high`, `gemini-3-flash`)
- Claude: `claude-<model>-<variant>` (예: `claude-sonnet-4-6`, `claude-opus-4-6-thinking`)
- GPT-OSS: `gpt-oss-<size>-<tier>` (예: `gpt-oss-120b-medium`)
- Internal: `chat_*`, `tab_*` (displayName 없음, 무시)
- displayName과 model_id가 실제로는 의도적으로 mismatch될 수 있음 (Antigravity 백엔드의 alias mapping). UI 표시는 항상 `displayName` 신뢰.

---

## 4. Legacy Fallback

agy 미설치 또는 비-Windows 사용자도 지원하려면 기존 경로를 fallback으로 유지:

```http
POST https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota
Authorization: Bearer <oauth_creds.json access_token>
Content-Type: application/json

{"project":"<projects.json의 project alias>"}
```

응답:
```json
{
  "buckets": [
    {
      "resetTime": "2026-05-25T16:45:31Z",
      "tokenType": "REQUESTS",
      "modelId": "gemini-2.5-flash",
      "remainingFraction": 1
    },
    ...
  ]
}
```

- 8 buckets (production + preview 포함, Claude/GPT-OSS는 없음)
- 모두 `tokenType: "REQUESTS"`, 동일한 daily reset cycle
- `remainingFraction`만 신뢰; `remainingAmount`는 100%일 때 응답에서 omit되는 버그 있음

**라우팅 로직**:
```
if wincred 'gemini:antigravity' 존재:
    fetch_antigravity()   # fetchAvailableModels
else:
    fetch_legacy()        # retrieveUserQuota
```

---

## 5. UI 모델 (4-Family 그룹화)

19 entries 그대로 보여주면 UI가 길어진다. claude_usage 위젯은 **4 family**로 묶고 family당 가장 소진된 변종 기준(보수적):

| family key | label | 분류 규칙 (model_id 기반) |
|---|---|---|
| `flash` | Flash | `contains("flash")` (flash-lite 포함) |
| `pro` | Pro | `contains("pro")` |
| `claude` | Claude | `starts_with("claude-")` |
| `gpt` | GPT-OSS | `starts_with("gpt-")` |

각 family에서 `min(remainingFraction)` 모델 선택 → utilization 계산. resetTime은 그 변종의 값을 사용.

legacy 응답엔 Claude/GPT-OSS 없으므로 Flash + Pro 2개 row만 보임 → UI가 "agy 미사용 사용자"를 자연스럽게 구분하는 신호로 활용 가능 (claude_usage는 이걸로 라벨을 'Gemini' vs 'Antigravity' 동적 전환).

---

## 6. 토큰 만료 / 갱신 운영

- access_token TTL은 1시간.
- agy는 자체적으로 refresh하므로, **agy를 한 번 실행하면 wincred entry가 새 access_token으로 갱신**된다. 가벼운 호출이면 충분: `agy --version`. 갱신 안 되면 prompt 실행: `agy -p "hi"`.
- agy는 oauth_creds.json은 안 만진다 — 이 파일의 mtime으로 갱신 성공을 판단하면 항상 실패로 인식. **wincred blob hash 변화를 success 신호로 봐야 한다**.
- 자체 OAuth flow를 구현해서 자체 refresh_token을 운영하면 agy와 독립. race condition 없음 (Google OAuth는 refresh_token rotation 안 함).

### 갱신 성공 판정 (둘 중 하나)

1. wincred `gemini:antigravity` blob hash가 agy 실행 전후로 다름
2. oauth_creds.json mtime이 갱신됨 (legacy gemini-cli 경로일 때만)

---

## 7. 알려진 함정

- `fetchAvailableModels`에 `Client-Metadata`, `X-Goog-Api-Client` 헤더 추가해도 403은 안 풀린다. 권한 부족 원인은 scope, 헤더가 아니다.
- 외부 블로그가 말하는 "sprint(5h) + weekly(7d) duail-limit"은 응답 스키마에 명시되지 않음. `resetTime` 분포(Gemini ~5h, Claude/GPT-OSS 다른 값)로 간접 추정만 가능.
- agy 토큰 발급 직후 expiry가 약 1시간이지만, 실제 사용자가 idle하면 갱신 안 됨. 위젯이 poll 시 401 받으면 그제야 cli_refresher 트리거하는 lazy 패턴이 자연스럽다.
- `cmdkey /list` / `vaultcmd /listcreds:"웹 자격 증명"` 에는 안 보인다. `Advapi32!CredEnumerateW(NULL, CRED_ENUMERATE_ALL_CREDENTIALS, ...)` 로 전체 dump해야 보인다.
- displayName과 model_id가 의도적으로 다를 수 있다 (`gemini-2.5-flash` → "Gemini 3.1 Flash Lite"). UI는 displayName만 보면 된다.

---

## 8. 참고 코드 / 출처

claude_usage 위젯의 통합 구현:
- [providers/antigravity_cred.rs](../src-tauri/src/providers/antigravity_cred.rs) — wincred reader
- [providers/gemini.rs](../src-tauri/src/providers/gemini.rs) — fetch_antigravity, fetch_legacy, model_tier, map_antigravity_to_response
- [cli_refresher.rs](../src-tauri/src/cli_refresher.rs) — token_state / token_refreshed (wincred hash + file mtime)
- [components/ProviderCard.tsx](../src/components/ProviderCard.tsx) — resolveLabel (동적 라벨 전환)

3rd-party 참고:
- [skainguyen1412/antigravity-usage](https://github.com/skainguyen1412/antigravity-usage) — OAuth/cloudcode 호출 코드
- [NoeFabris/opencode-antigravity-auth](https://github.com/NoeFabris/opencode-antigravity-auth/blob/main/docs/ANTIGRAVITY_API_SPEC.md) — API spec
- [steipete/CodexBar antigravity docs](https://github.com/steipete/CodexBar/blob/main/docs/antigravity.md) — local IDE language server 접근 방법
