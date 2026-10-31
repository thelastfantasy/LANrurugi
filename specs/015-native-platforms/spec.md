# Feature Specification: Native multi-platform builds and releases

**Feature Branch**: `015-native-platforms`

**Created**: 2026-10-19

**Status**: Draft

**Input**: User description: "原生多平台运行与发布：除现有 Docker 镜像外，让 LANrurugi 能以原生方式运行在 Linux 各发行版（x86_64 / aarch64）、macOS（Apple Silicon 为主）与 FreeBSD（x86_64）上，并发布可直接下载的原生安装包；Windows 本期不做；鸿蒙本期只做可行性评估与文档说明。"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Run natively on a Linux machine without containers (Priority: P1)

A user on a Linux server or desktop (any mainstream distribution, x86_64 or ARM64) who does not want
to run containers downloads one release package, installs the few system packages the install guide
lists (database server, plugin runtime, fonts, DjVu tools), unpacks the package, starts the service and
opens the web interface. Their existing library and reading features work as they do in the container.

**Why this priority**: Linux is the platform most self-hosters use and the one the code already
targets; a native package there delivers the most value for the least risk, and it proves the package
layout every other platform reuses.

**Independent Test**: On a clean Linux system with only the listed system packages installed, unpack
the package, start the service with no configuration other than the library location, open the web
interface, read an archive, a PDF and a text e-book, and run a plugin.

**Acceptance Scenarios**:

1. **Given** a clean Linux system with the listed packages, **When** the user unpacks the release
   package and starts the service pointing at a library directory, **Then** the web interface loads,
   the library is scanned and archives open in the reader without any further configuration.
2. **Given** the service was started from the unpacked package, **When** the user opens a PDF, an EPUB
   or a DjVu book, **Then** pages render as they do in the container image.
3. **Given** the plugin runtime is installed, **When** a metadata plugin runs, **Then** it behaves as in
   the container.
4. **Given** the user installed the provided service definition, **When** the machine reboots, **Then**
   the service starts on its own.

---

### User Story 2 - Run natively on an Apple Silicon Mac (Priority: P1)

A Mac user (Apple Silicon) installs the database server, plugin runtime and DjVu tools with the common
macOS package manager, downloads the macOS package, starts the service and reads their library in the
browser, including manga translation on the CPU and text e-books typeset with the system's own CJK
fonts.

**Why this priority**: The user named macOS on ARM as a priority; Macs are a common home for personal
libraries, and Docker on macOS runs in a virtual machine with a real cost in memory and file access.

**Independent Test**: On an Apple Silicon Mac, follow the macOS section of the install guide, start the
service, read an archive and a Chinese/Japanese text e-book, and run translation on a page.

**Acceptance Scenarios**:

1. **Given** an Apple Silicon Mac with the listed packages, **When** the user starts the service from
   the macOS package, **Then** the web interface loads and the library opens.
2. **Given** no extra fonts were installed, **When** a Chinese or Japanese text e-book is opened,
   **Then** its pages are typeset with a CJK font the system already has.
3. **Given** translation is enabled, **When** a page is translated, **Then** it completes on the CPU
   without requiring a graphics accelerator.

---

### User Story 3 - Run natively on FreeBSD, with honest limits (Priority: P2)

A FreeBSD user installs the listed packages, unpacks the FreeBSD package and runs the service. Features
whose native components have no FreeBSD build are reported as unavailable instead of crashing the
service, and the install guide states in advance which features those are.

**Why this priority**: BSD was named as a priority, but several third-party components have no FreeBSD
builds; the value is a working core library and reader with clearly stated gaps.

**Independent Test**: On FreeBSD, start the service, read archives and EPUB/text books, and confirm the
features without FreeBSD support (listed in the guide) report a clear "not available on this system"
reason.

**Acceptance Scenarios**:

1. **Given** FreeBSD with the listed packages, **When** the service starts, **Then** the library,
   reader, search and archive formats work.
2. **Given** a component is not available on FreeBSD (for example the PDF renderer), **When** the user
   opens a PDF, **Then** the reader reports that PDF support is unavailable on this system, and the
   rest of the library keeps working.

---

### User Story 4 - Know which platforms and features are supported (Priority: P2)

A prospective user visiting the homepage or the install guide sees every supported way to run
LANrurugi — container, native Linux, native macOS, native FreeBSD — what each needs, and which features
work on each platform, before downloading anything. HarmonyOS users find a plain statement of the
current status and why.

**Why this priority**: Without this, native packages exist but nobody can tell whether their system is
supported or what they would give up.

**Independent Test**: Read the homepage start section and the install guide and answer, for each
platform, "can I run it, what do I install first, and which features do I lose".

**Acceptance Scenarios**:

1. **Given** the homepage, **When** a visitor reads the start section, **Then** it names the container
   route and the native packages for Linux, macOS and FreeBSD, and links to per-platform instructions.
2. **Given** the install guide, **When** a FreeBSD user reads it, **Then** it lists the packages to
   install and the features that are unavailable on FreeBSD.
3. **Given** the install guide, **When** a HarmonyOS user reads it, **Then** it states that HarmonyOS is
   not supported yet and why.

---

### User Story 5 - Release packages are produced automatically (Priority: P3)

When the maintainer tags a release, packages for every supported platform are built, checked and
attached to the release without manual steps, next to the container images.

**Why this priority**: Hand-built packages drift and get skipped; automation is what keeps native
packages alive after the first release.

**Independent Test**: Tag a release on a fork and confirm the release receives one package per
platform, each starting successfully in its build job's smoke test.

**Acceptance Scenarios**:

1. **Given** a release tag, **When** the release automation finishes, **Then** the release lists one
   package per supported platform and architecture with a checksum file.
2. **Given** a platform build fails, **When** the automation runs, **Then** the failure is visible and
   no incomplete package is published for that platform.

### Edge Cases

- The service is started from a symbolic link or a different working directory: the installation's own
  resources must still be found.
- An optional component (PDF renderer, inference runtime, DjVu tools, plugin runtime, models) is
  missing: the service starts, logs once which features are unavailable, and each affected feature
  reports the reason when used.
- The installed database server is older than the on-disk data from a previous container deployment:
  the guide states the minimum version, and the service reports a clear error instead of corrupting
  data.
- A Mac or BSD machine has no Noto CJK fonts: text books fall back to the system's own CJK fonts.
- The container image is built for ARM64: every bundled native library matches the image architecture.
- A user overrides a resource location explicitly: the explicit setting always wins over discovery.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST run natively on Linux (x86_64 and aarch64, mainstream glibc-based
  distributions), macOS on Apple Silicon (arm64) and FreeBSD (x86_64).
- **FR-002**: No feature may depend on Linux-only operating-system interfaces without an equivalent or
  a graceful fallback on the other supported systems.
- **FR-003**: When resource locations are not configured, the service MUST find its web interface,
  plugins, documentation, helper programs, bundled native libraries and models relative to its own
  installation, independent of the working directory; explicit configuration MUST take precedence.
- **FR-004**: Each release package MUST contain the service and its helper programs, the web interface,
  plugins, documentation, the native libraries that may be redistributed for that platform, a model
  download script, and a service definition example for that platform's service manager.
- **FR-005**: Components the user must install from the system package manager (database server,
  plugin runtime, DjVu tools, CJK fonts) MUST be listed per platform in the install guide, with the
  exact install command and any minimum version.
- **FR-006**: When an optional component is missing, the service MUST start, and every feature that
  needs it MUST report that it is unavailable on this system rather than failing silently or crashing.
- **FR-007**: Text e-books MUST be typeset with an available CJK font on each platform, including the
  fonts macOS ships with.
- **FR-008**: Translation and other inference features MUST work on the CPU on macOS; hardware
  acceleration outside Linux is not required.
- **FR-009**: Release automation MUST build, smoke-test and publish one package per supported platform
  and architecture, with checksums, whenever a release is tagged.
- **FR-010**: The container image for ARM64 MUST bundle native libraries built for ARM64.
- **FR-011**: The homepage and the install guide MUST describe every supported way to run the system,
  per-platform prerequisites and per-platform feature availability, in every language they are
  published in.
- **FR-012**: The documentation MUST state the HarmonyOS status and the reasons it is not supported
  yet.

### Key Entities

- **Release package**: one archive per platform and architecture; its fixed directory layout is what
  resource discovery relies on.
- **Platform capability**: for each platform, which optional components are bundled, installable or
  unavailable, and therefore which features work.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On a clean Linux system, a user goes from download to reading their library in under 10
  minutes following the guide, setting at most one location (the library) by hand.
- **SC-002**: Every release publishes packages for all four platform/architecture combinations (Linux
  x86_64, Linux aarch64, macOS arm64, FreeBSD x86_64), each passing a start-up smoke test.
- **SC-003**: 100% of features unavailable on a platform are listed in the guide and report the reason
  when used; none crashes the service.
- **SC-004**: Reading archives and e-books natively gives the same pages as the container image on the
  same files.
- **SC-005**: The ARM64 container image's inference-based features (recommendations, OCR) work, where
  they previously could not load.

## Assumptions

- Linux packages target glibc-based distributions; musl-based ones (e.g. Alpine) are served by the
  container image.
- The database server, plugin runtime, DjVu tools and fonts come from each platform's package manager
  (apt/dnf/pacman, Homebrew, pkg); they are not bundled.
- Machine-learning models are downloaded by a script after install, not shipped inside packages (size).
- Hardware-accelerated inference stays Linux-only; other platforms use the CPU.
- macOS Intel and other BSDs are not built or tested; the guide says so.
- Windows users use the container image (Docker Desktop); native Windows is out of scope.
- HarmonyOS/OpenHarmony: assessment and documentation only in this feature.
