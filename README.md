# Unreal Hub

엔진 버전 관리와 프로젝트 열기에만 집중한, 빠르고 가벼운 언리얼 엔진 런처입니다. Rust와 [GPUI](https://www.gpui.rs/)로 만들었습니다.

## 주요 기능

- **프로젝트 목록:** Epic Games Launcher의 프로젝트 폴더, 에디터 최근 프로젝트, 직접 추가한 폴더를 하나의 목록으로 합쳐 보여 줍니다.
- **엔진 자동 감지:** 런처로 설치한 엔진과 소스 빌드 엔진을 찾아 각 프로젝트가 어떤 엔진을 쓰는지 표시합니다.
- **클릭 한 번으로 실행:** 프로젝트를 클릭하면 맞는 엔진의 에디터를 실행하고, 실행 중인 프로젝트를 표시합니다.
- **프로젝트 관리:** 새 프로젝트 생성, 엔진 버전 변경(`.uproject` 백업 포함), 캐시 정리(휴지통으로 이동).
- **편의 기능:** 한국어·영어 UI, 시스템 트레이, Windows 시작 시 자동 실행, GitHub Releases 기반 자동 업데이트.

## 요구 사항

- Windows 10 이상 (x64)
- Unreal Engine 5.6 ~ 5.9 (그 이전 버전도 감지·실행은 됩니다)

## 설치

[Releases](https://github.com/Mossworm/unreal-hub/releases) 페이지에서 `UnrealHub-<버전>-setup.exe`를 내려받아 실행합니다.

- 관리자 권한 없이 `%LOCALAPPDATA%\Programs\Unreal Hub`에 설치됩니다.
- 사용자 데이터는 `%APPDATA%\UnrealHub`에 저장되며, 제거해도 남습니다.
- 설치 파일은 코드 서명되어 있지 않아 Windows SmartScreen 경고가 뜰 수 있습니다. [추가 정보] → [실행]을 눌러 진행합니다.

설치 후에는 새 버전이 나오면 앱 안에서 업데이트 배너로 알려 줍니다.

## 소스에서 빌드

[Rust](https://rustup.rs/) 안정 버전이 필요합니다.

```bash
cargo build --release
```

개발 중 검사:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

설치 파일을 만들려면 [Inno Setup 6](https://jrsoftware.org/isinfo.php)이 필요합니다 (`winget install JRSoftware.InnoSetup`).

```bash
powershell -ExecutionPolicy Bypass -File scripts/build-installer.ps1
```

결과물은 `dist\UnrealHub-<버전>-setup.exe`입니다.

## 릴리스

`v1.2.3` 형식의 태그를 push하면 GitHub Actions가 검사·빌드 후 설치 파일을 첨부한 **초안 릴리스**를 만듭니다. 릴리스 노트를 다듬은 뒤 정식 릴리스로 게시해야 자동 업데이트에 잡힙니다. 자세한 절차는 [docs/RELEASE.md](docs/RELEASE.md)를 참고하세요.

## 라이선스

[MIT](LICENSE)
