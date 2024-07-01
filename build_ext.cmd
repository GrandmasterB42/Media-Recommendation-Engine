@echo off

:: Check if Cargo is installed
where cargo >nul 2>&1
if %errorlevel% neq 0 (
    echo Please install cargo
    exit 1
    ::echo Installing cargo...
    ::curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
)

:: Check if wasm-pack is installed
where wasm-pack >nul 2>&1
if %errorlevel% neq 0 (
    echo Installing wasm-pack...
    cargo install wasm-pack
)

:: Clean up existing 'pkg' directory
rmdir /s /q pkg

:: Check if '--release' flag was passed
set release=
for %%i in (%*) do (
    if "%%i" == "--release" (
        set release=true
    )
)

:: Build for release or debug
echo Starting the build process...
if defined release (
    wasm-pack build chrome_extension --target=no-modules --release --out-dir="../pkg" || exit /b 1
) else (
    wasm-pack build chrome_extension --target=no-modules --dev --out-dir="../pkg" || exit /b 1
)

:: Copy necessary files to pkg
copy chrome_extension\manifest.json pkg\manifest.json
copy chrome_extension\run_wasm.js pkg\run_wasm.js
