# ParentView Next

독립 Media SDK와 ParentView 서비스를 새로 구축하는 모노레포입니다. 이전 ParentView는 deprecated 참고 자료이며 데이터 이관과 구버전 호환을 제공하지 않습니다.

**Media SDK는 기능과 peer만 다룹니다. `parent`·`child`, 가족 관계, 도움 요청, 동의와 제어권은 ParentView 서비스 계층의 책임입니다.** [필수 개발 규칙](AGENTS.md)과 [레이어별 소유권](docs/architecture.md)을 먼저 읽어 주세요.

## 현재 제공하는 기반

- `packages/media-sdk`: 독립 ESM/타입 선언 빌드, Rx 공개 API, 역할 없는 4인 mesh, 브라우저 WebRTC 어댑터.
- `packages/parentview-services`: 명시적 역할 정책, 세션·화면 동의·배타적 제어권, Network/Device 서비스와 주입 가능한 포트. 실제 OS 입력 구현은 포함하지 않습니다.
- `services/signaling`: Rust/Tokio/Axum 기기 인증, 방 초대, 동일 방 시그널 전달, 정원·만료·큐 제한, coturn 임시 자격 증명.
- `apps/desktop`: 실제 카메라·음성·화면·데이터 전송을 확인하는 개발자 실험실과 Tauri 호스트. 완성된 부모·자녀 서비스 UI가 아닙니다.
- `scripts/architecture.ts`: SDK의 역할 침투, 플랫폼 의존, 역방향 import와 모듈 순환을 검사합니다. SDK 코어는 DOM 타입 없이 별도로 컴파일합니다.

## 실행

Node.js 22.12+, Bun 1.1.24+, Rust 1.88+, 대상 OS의 Tauri 개발 도구가 필요합니다. 로컬 검증 환경은 Node.js 22.19.0, Bun 1.1.24, Rust 1.88.0입니다. `bun.lockb`와 `Cargo.lock`을 커밋합니다. 개발·실험실은 로컬 서버 기준입니다.

```sh
bun install --frozen-lockfile

# 터미널 1: 인증/시그널링 서버
bun run dev:server

# 터미널 2: 브라우저 검증 화면
bun run dev

# 또는 같은 UI를 Tauri 앱으로 실행
bun run dev:desktop
```

`http://localhost:1420`에서 새 실험실을 만들고 초대 링크를 다른 탭에 붙여 넣습니다. 탭마다 별도 기기 신원을 발급받습니다. 최대 4개 탭에서 카메라·마이크를 켜고 영상 수신과 메시지를 확인할 수 있습니다. 화면 캡처는 현재 브라우저/OS 권한에 따릅니다. 현재 초대는 **일시적인 미디어 방 초대**이며 장기 가족 페어링이 아닙니다.

초대 링크의 fragment에는 방 입장 권한이 들어 있습니다. 서버 로그·쿼리·영구 저장소에 토큰을 기록하지 않으며, 첫 페이지 로딩 시 주소에서 fragment를 제거합니다. 기기 인증 정보는 실험실 탭 메모리에만 보관합니다. 실험실 서버를 재시작하면 기기와 방 정보가 사라집니다.

## 검증

```sh
bun run check
cargo test -p parentview-signaling
cargo clippy -p parentview-signaling --all-targets -- -D warnings
cargo fmt --all --check
bun run build:web
bun run test:e2e
```

브라우저 E2E는 설치된 Google Chrome을 별도 테스트 프로필로 실행하고 가상 카메라·마이크를 사용합니다. 실제 WebRTC 연결과 인코딩/디코딩, 데이터 채널을 검증하지만 실제 하드웨어 캡처 품질 검증을 대신하지 않습니다. 로컬 1420/8787 포트가 비어 있어야 합니다. 테스트가 자기 서버를 시작하고 종료합니다. 캡처 증거는 Git에서 제외한 `proof/`에 저장됩니다.

## coturn 컨테이너 템플릿

현재 Compose는 **설정 문법만 검증한 로컬 컨테이너 템플릿**입니다. 실제 TURN relay 연결은 아직 검증하지 않았습니다. 특히 macOS Docker VM에서는 컨테이너가 광고하는 relay 주소와 호스트의 포트 전달 주소를 일치시키는 `external-ip` 구성이 추가로 필요합니다.

```sh
# 터미널 세션용 비밀: 출력하거나 저장소에 넣지 않습니다.
export TURN_SHARED_SECRET="$(openssl rand -hex 32)"
docker compose -f infra/compose.yaml config --quiet
docker compose -f infra/compose.yaml up -d
```

컨테이너 기동만으로 relay 경로가 완성되지는 않습니다. [TURN 네트워크 구성과 검증 조건](infra/README.md)을 충족한 뒤 시그널링 서버에 `TURN_URLS`를 설정합니다. 서버와 coturn은 같은 비밀을 사용하며 클라이언트에는 시간 제한 HMAC 자격 증명만 전달합니다. TURN이 설정되지 않으면 host candidate만으로 로컬 연결을 시험합니다. 로컬 성공을 인터넷/TURN 연결 성공으로 해석하지 않습니다.

## 다음 구현 단계

1. 네이티브 libwebrtc 엔진과 Android·iOS·Windows·macOS 캡처/렌더 어댑터. 원본 영상은 JS JSON IPC로 보내지 않습니다.
2. 영속적인 기기 인증·QR/코드 가족 페어링, 부모·자녀 서비스 UI와 도움 요청/승인 프로토콜.
3. Android·Windows·macOS의 실제 원격 입력과 수신 측 권한 검사. iOS 기기 전체 입력 제어는 지원한다고 가정하지 않습니다.
4. 시그널링 복구·ICE restart, 네트워크 전환, TURN 강제 경로, 화면+카메라 동시 송출 성능과 실기기 검증.

Tauri 호스트는 현재 `nativeMedia: false`, `remoteInput: false`를 반환합니다. OS별 기능 지원 여부는 실제 구현과 검증 후에만 바꿉니다. 현재 SDK는 시그널링이 끊기면 세션을 정리하며 자동 재접속하지 않습니다. 현재 서버는 공개 운영 서비스용 인증·배포 구성이 아닙니다.

## 설계 기준

- [아키텍처와 소유권](docs/architecture.md)
- [시그널링/SDK 계약](docs/protocol.md)
- [초기 구현 계획](docs/plans/2026-09-28-bootstrap.md)
- [검증 결과와 남은 범위](docs/verification.md)
- [SDK 사용과 종료 계약](packages/media-sdk/README.md)
- [시그널링 설정과 운영 제한](services/signaling/README.md)
- [검증 UI와 호스트](apps/desktop/README.md)
- 코드 스타일: [ws-client-pack](https://github.com/4sizn/ws-client-pack/tree/ceb533ad100acacd83f125a9789da3d528dae27b)
