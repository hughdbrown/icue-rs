# EdgeHost — test, build, bundle and deploy.
#
# Usage:
#   just                — list available recipes
#   just ci             — everything CI would run (Rust + JS tests, clippy, fmt)
#   just deploy         — run the checks, then install edgectl and EdgeHost.app
#   just run-edgehost   — run the dashboard from the dev tree

# List available recipes
default:
    @just --list

# ── Variables ──────────────────────────────────────────────

app_name := "EdgeHost"
bundle_id := "com.github.hughdbrown.edgehost"
min_macos := "14.0"
release_dir := "target/release"
bundle_dir := release_dir / "bundle"
app_bundle := bundle_dir / app_name + ".app"
# Where `just deploy` puts EdgeHost.app; `just app_dir=/Applications deploy-app` for all users.
app_dir := env_var("HOME") / "Applications"
dist_dir := "dist"
version := `grep -m1 '^version' Cargo.toml | cut -d'"' -f2`

# Directories holding `node --test` suites (*.test.mjs).
js_test_dirs := "crates/widget-runtime/shim apps/edgehost/ui examples/widgets/git examples/widgets/piano examples/widgets/portfolio examples/widgets/warthunder-hud"

# ── Development ────────────────────────────────────────────

# Fast type-check of the whole workspace
check:
    cargo check --workspace --all-targets

# Run edgectl from the dev tree, e.g. `just edgectl validate examples/widgets/hello-edge`
edgectl *args:
    cargo run -p edgectl -- {{ args }}

# Run the EdgeHost dashboard from the dev tree, e.g. `just run-edgehost --display windowed`
run-edgehost *args:
    cargo run -p edgehost -- {{ args }}

# Run one widget, e.g. `just run-one examples/widgets/hello-edge --slot L`
run-one widget *args:
    cargo run -p edgectl -- run-one {{ widget }} {{ args }}

# Build the API docs
doc:
    cargo doc --workspace --no-deps

# ── Testing ────────────────────────────────────────────────

# Run every test suite: Rust and JavaScript
test: test-rust test-js

# Rust tests for the whole workspace
test-rust:
    cargo test --workspace

# JavaScript tests: the injection shim, the EdgeHost UI and the example widgets
test-js:
    #!/usr/bin/env bash
    set -euo pipefail
    for dir in {{ js_test_dirs }}; do
        echo "── node --test $dir"
        node --test "$dir/"
    done

# Rust tests matching a filter, e.g. `just test-one manifest`
test-one filter:
    cargo test --workspace {{ filter }}

# ── Linting ────────────────────────────────────────────────

# Lint with clippy, warnings as errors
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Check formatting without modifying files
fmt-check:
    cargo fmt --all -- --check

# Reformat the workspace
fmt:
    cargo fmt --all

# Everything CI would run: format check, clippy, all tests
ci: fmt-check clippy test
    @echo ""
    @echo "All checks passed."

# ── Build ──────────────────────────────────────────────────

# Debug build of the workspace
build:
    cargo build --workspace

# Optimised build of edgectl and edgehost
release:
    cargo build --release -p edgectl -p edgehost

# Wrap the release edgehost binary in EdgeHost.app (ad-hoc signed)
bundle: release
    #!/usr/bin/env bash
    set -euo pipefail
    app="{{ app_bundle }}"
    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp {{ release_dir }}/edgehost "$app/Contents/MacOS/edgehost"
    cat > "$app/Contents/Info.plist" <<'PLIST'
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
        <key>CFBundleName</key>                 <string>{{ app_name }}</string>
        <key>CFBundleDisplayName</key>          <string>{{ app_name }}</string>
        <key>CFBundleIdentifier</key>           <string>{{ bundle_id }}</string>
        <key>CFBundleExecutable</key>           <string>edgehost</string>
        <key>CFBundlePackageType</key>          <string>APPL</string>
        <key>CFBundleShortVersionString</key>   <string>{{ version }}</string>
        <key>CFBundleVersion</key>              <string>{{ version }}</string>
        <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
        <key>LSMinimumSystemVersion</key>       <string>{{ min_macos }}</string>
        <key>NSHighResolutionCapable</key>      <true/>
    </dict>
    </plist>
    PLIST
    plutil -lint "$app/Contents/Info.plist" >/dev/null
    # Ad-hoc signature: enough for a local machine, and gives macOS a stable identity to attach
    # permissions (Input Monitoring, Automation) to. Distribution needs Developer ID + notarization.
    codesign --force --sign - --identifier {{ bundle_id }} "$app"
    echo "Built $app"

# ── Deploy ─────────────────────────────────────────────────

# Run all checks, then install edgectl and EdgeHost.app
deploy: ci deploy-cli deploy-app

# Install edgectl into ~/.cargo/bin
deploy-cli:
    cargo install --path apps/edgectl --locked --force

# Install EdgeHost.app into app_dir (default ~/Applications), quitting a running copy first
deploy-app: bundle
    #!/usr/bin/env bash
    set -euo pipefail
    target="{{ app_dir }}/{{ app_name }}.app"
    # Only the installed copy is stopped; a dev-tree `cargo run` instance is left alone.
    installed_exe="$target/Contents/MacOS/edgehost"
    if pgrep -qf "^$installed_exe"; then
        echo "Quitting the installed EdgeHost"
        pkill -f "^$installed_exe" || true
        sleep 1
    fi
    mkdir -p "{{ app_dir }}"
    rm -rf "$target"
    ditto "{{ app_bundle }}" "$target"
    echo "Installed $target"
    echo "Launch it with: open \"$target\""

# Remove the installed edgectl and EdgeHost.app (keeps the data folder)
uninstall:
    -cargo uninstall edgectl
    rm -rf "{{ app_dir }}/{{ app_name }}.app"

# Zip EdgeHost.app and edgectl into a versioned archive in dist/
dist: ci bundle
    #!/usr/bin/env bash
    set -euo pipefail
    stage="{{ dist_dir }}/{{ app_name }}-{{ version }}"
    rm -rf "$stage"
    mkdir -p "$stage"
    ditto "{{ app_bundle }}" "$stage/{{ app_name }}.app"
    cp {{ release_dir }}/edgectl README.md "$stage/"
    archive="$stage-macos-$(uname -m).zip"
    rm -f "$archive"
    ditto -c -k --keepParent "$stage" "$archive"
    rm -rf "$stage"
    ls -lh "$archive"

# ── Housekeeping ───────────────────────────────────────────

# Remove build artifacts and built archives
clean:
    cargo clean
    rm -rf {{ dist_dir }}
