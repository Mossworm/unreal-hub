# Unreal Hub 배포 준비 문서

- 작성일: 2026-11-16
- 기준 커밋: `7de6366` (M1~M6 구현 완료, `main`)
- 현재 버전: `0.1.0` (`Cargo.toml`)
- 목표: GitHub Release v1.0.0 (PRD §13 M6 완료 기준)

M1~M6 기능 구현은 끝났지만 실제 배포까지는 아래 과정이 남아 있다. 순서대로 진행하는 것을 기준으로 정리했다.

---

## 1. 저장소와 기본 문서 (필수)

| 항목 | 내용 | 상태 |
|---|---|---|
| GitHub 저장소 생성 | `Mossworm/unreal-hub` 이름으로 만들고 push한다. 업데이트 기능(`src/updates.rs`의 `REPOSITORY`)이 이 저장소를 본다. | 미완료 |
| 공개 저장소 | 업데이트 확인은 인증 없이 GitHub API(`/repos/{저장소}/releases/latest`)를 호출하므로 저장소가 **공개**여야 동작한다. | 미완료 |
| 라이선스 결정 | MIT로 정하고 `LICENSE`를 추가했다. `Cargo.toml`에도 `license = "MIT"`를 적었다. | 완료 |
| README 작성 | `README.md`에 설치 방법, 지원 엔진 버전(UE 5.6~5.9), 주요 기능, 빌드·릴리스 방법을 정리했다. | 완료 |

저장소 이름을 바꾸면 다음 세 곳을 함께 고쳐야 한다.
- `src/updates.rs`의 `REPOSITORY` 상수
- `installer/unreal-hub.iss`의 `AppPublisherURL`, `AppSupportURL`, `AppUpdatesURL`
- `Cargo.toml`의 `repository`, `README.md`의 Releases 링크

---

## 2. 미검증 기능 확인 (필수)

구현 과정에서 자동 테스트와 화면 캡처로 확인하지 못한 항목이다. 배포 전에 직접 확인한다.

### 2.1 설치 파일
- [ ] 설치 파일을 실행해 `%LOCALAPPDATA%\Programs\Unreal Hub`에 관리자 권한 없이 설치되는지
- [ ] 시작 메뉴 바로가기(그리고 선택 시 바탕화면 바로가기)가 생기는지
- [ ] 설치 화면이 한국어/영어로 표시되는지
- [ ] 제거 시 프로그램이 지워지고, Windows 시작 프로그램 등록(`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`의 `UnrealHub`)도 지워지는지
- [ ] 제거 후에도 사용자 데이터(`%APPDATA%\UnrealHub`)는 남는지 (의도된 동작)

### 2.2 자동 업데이트 전체 흐름
첫 릴리스 이후 두 번째 릴리스를 낼 때 확인할 수 있다.
- [ ] 이전 버전이 설치된 상태에서 새 릴리스를 게시하면 시작 시 업데이트 배너가 뜨는지
- [ ] [업데이트] → 다운로드 진행률 → [재시작하여 업데이트] → 조용한 설치 → Hub 재실행까지 이어지는지
- [ ] [나중에]를 누르면 해당 버전 배너가 다음 실행 전까지 숨겨지는지
- [ ] 설정 → 정보 → [업데이트 확인]으로 수동 확인이 되는지

### 2.3 자동 시작과 트레이
- [ ] 설정 → 일반 → "Windows 시작 시 Unreal Hub 실행"을 켜고 재부팅하면 실행되는지
- [ ] "시작 시 최소화 상태로 실행"을 켜면 창 없이 트레이로 시작되는지
- [ ] 트레이 아이콘 좌클릭으로 창이 열리는지
- [ ] 트레이 우클릭 메뉴의 최근 프로젝트 실행, "Unreal Hub 열기", "종료"가 동작하는지

### 2.4 클릭으로만 확인할 수 있는 기능
- [ ] 프로젝트 클릭 → 에디터 실행 → "실행 중" 배지 표시/해제
- [ ] 엔진 버전 변경 다이얼로그 (`.uproject.bak` 백업 포함)
- [ ] 캐시 정리 다이얼로그 (휴지통으로 이동 후 복원 가능 여부)
- [ ] 새 프로젝트 다이얼로그에서 실제로 프로젝트를 끝까지 생성하고 목록에 추가·실행되는지

### 2.5 엔진 버전
- [ ] UE 5.7, 5.9에서 엔진 감지, 프로젝트 실행, 새 프로젝트 생성 확인 (지금까지 5.6과 5.8만 검증)

새 프로젝트 생성은 설치된 엔진으로 직접 시험하는 테스트가 있다.

```bash
UNREAL_HUB_TEST_ENGINE="C:\Program Files\Epic Games\UE_5.9" UNREAL_HUB_TEST_OUTPUT="D:\HubTest" cargo test creates_projects_from_installed_engine -- --ignored --nocapture
```

출력 경로는 짧게 둔다. 프로젝트 경로 길이 제한(130자)에 걸릴 수 있다.

---

## 3. 버전과 빌드 (필수)

1. `Cargo.toml`의 `version`을 `1.0.0`으로 올리고 커밋한다. (올림 완료, 커밋 전)
2. 배포 전 검사를 모두 통과시킨다.
   ```bash
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```
3. 설치 파일을 만든다. 릴리스 빌드 후 Inno Setup으로 `dist\UnrealHub-<버전>-setup.exe`를 만들고 SHA-256을 출력한다.
   ```bash
   powershell -ExecutionPolicy Bypass -File scripts/build-installer.ps1
   ```

참고
- 빌드에는 Inno Setup 6이 필요하다 (`winget install JRSoftware.InnoSetup`).
- 현재 `dist\`에 있는 `UnrealHub-0.1.0-setup.exe`는 마지막 수정 전에 빌드된 것이므로 배포에 쓰지 않는다.

---

## 4. 코드 서명 (권장, PRD §13 미결)

서명하지 않은 설치 파일은 Windows SmartScreen에서 "알 수 없는 게시자" 경고가 뜬다.

| 선택지 | 비용 | 특징 |
|---|---|---|
| 서명하지 않음 | 없음 | 경고가 뜬다. 개인 프로젝트 초기에 흔한 방식 |
| Azure Trusted Signing | 월 단위 소액 | 클라우드 서명. 개인 가입 조건 확인 필요 |
| OV/EV 코드 서명 인증서 | 연 단위 | EV는 SmartScreen 평판이 바로 쌓임 |

서명을 하게 되면 Inno Setup의 `SignTool` 설정으로 설치 파일과 `unreal-hub.exe`를 함께 서명한다.

---

## 5. GitHub Release 게시 (필수)

| 항목 | 규칙 |
|---|---|
| 태그 | `v1.0.0` 형식 (`v` + Major.Minor.Patch) |
| 자산 | `UnrealHub-1.0.0-setup.exe` 첨부. 업데이트 기능은 이름이 `-setup.exe`로 끝나는 자산을 찾는다 |
| 체크섬 | GitHub가 자산마다 SHA-256을 제공하며, Hub는 내려받은 뒤 이 값으로 검증한다 |
| 릴리스 종류 | **정식 릴리스**로 게시한다. 초안(draft)과 사전 릴리스(pre-release)는 업데이트 확인에서 보이지 않는다 |
| 릴리스 노트 | 주요 기능, 알려진 제한 사항, 지원 엔진 버전을 적는다 |

### 5.1 자동화된 절차 (GitHub Actions)

`.github/workflows/`에 두 워크플로가 있다.

| 워크플로 | 실행 시점 | 하는 일 |
|---|---|---|
| `ci.yml` | `main` push, PR | `cargo fmt --check`, `clippy -D warnings`, `cargo test` (windows-latest) |
| `release.yml` | `v*.*.*` 태그 push | 태그와 `Cargo.toml` 버전 일치 확인 → fmt/clippy/test → Inno Setup 설치 → `scripts/build-installer.ps1` → 설치 파일을 첨부한 **초안 릴리스** 생성 |

릴리스 순서:

```bash
git tag v1.0.0
git push origin v1.0.0
```

워크플로가 끝나면 GitHub Releases의 초안에서 릴리스 노트를 다듬고 **Publish release**를 눌러 정식 릴리스로 게시한다. 초안 상태로는 업데이트 확인에 잡히지 않는다. 태그와 `Cargo.toml` 버전이 다르면 워크플로가 실패하므로 버전을 먼저 올려 커밋한 뒤 태그를 만든다.

---

## 6. 있으면 좋은 것 (선택)

- **GitHub Actions 릴리스 워크플로:** 추가 완료. §5.1 참고.
- **PRD 미결 사항 확정:** 구현 중 기본값으로 정해 둔 부분을 PRD에 기록한다.
  - §14.1 새 프로젝트 생성 방식: 에디터의 `GameProjectUtils::CreateProjectFromTemplate` 로직을 재현. 5.6, 5.8에서 BP/C++ 생성과 C++ 빌드 성공 확인
  - §14.3 설치 파일 형식: Inno Setup EXE(사용자 단위 설치)로 결정. 코드 서명은 미정
  - §14.4 5.6 미만 엔진: 현재는 감지·실행·생성 모두 제한 없이 허용
  - §14.5 다국어 문구: 코드 안에 한국어·영어를 나란히 두는 `tr!`/`trf!` 매크로 방식. 언어가 늘면 Fluent 같은 별도 파일 방식으로 옮기는 것을 검토
- **PRD와 다르게 구현한 부분 기록:**
  - 새 프로젝트의 스타터 콘텐츠·레이트레이싱 토글 제외 (UE 5.6/5.8 에디터에서 제거·항상 적용되는 항목)
  - 템플릿 배리언트(3인칭의 전투·플랫포머 등) 미지원

---

## 체크리스트 요약

- [ ] GitHub 공개 저장소 `Mossworm/unreal-hub` 생성 및 push
- [x] 라이선스 결정 및 `LICENSE` 추가 (MIT)
- [x] README 작성
- [ ] 미검증 기능 확인 (설치·제거, 자동 시작·트레이, 클릭 동작, 5.7/5.9)
- [x] 버전 1.0.0으로 올리기
- [ ] fmt / clippy / test 통과
- [ ] 설치 파일 빌드 (태그 push 시 Release 워크플로가 수행, 로컬은 `scripts/build-installer.ps1`)
- [ ] 코드 서명 여부 결정 (선택 시 서명 적용)
- [ ] GitHub Release `v1.0.0` 정식 게시 (설치 파일 첨부)
- [ ] 두 번째 릴리스 때 자동 업데이트 전체 흐름 확인
