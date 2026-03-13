# Build and install binaries to ~/.local/bin
install:
    cargo build --release -p lore-mcp -p lore-daemon -p lore-tray
    cp target/release/lore-{mcp,daemon,tray} ~/.local/bin/

# Build macOS .app bundle at target/Lore.app
bundle-macos:
    cargo build --release -p lore-tray -p lore-daemon
    rm -rf target/Lore.app
    mkdir -p target/Lore.app/Contents/MacOS
    mkdir -p target/Lore.app/Contents/Resources
    cp lore-tray/macos/Info.plist target/Lore.app/Contents/
    cp lore-tray/macos/AppIcon.icns target/Lore.app/Contents/Resources/
    cp target/release/lore-tray target/Lore.app/Contents/MacOS/
    cp target/release/lore-daemon target/Lore.app/Contents/MacOS/
    @echo "Bundle created at target/Lore.app"

# Install on Linux with .desktop entry and icon
install-linux:
    cargo build --release -p lore-mcp -p lore-daemon -p lore-tray
    mkdir -p ~/.local/bin
    cp target/release/lore-{mcp,daemon,tray} ~/.local/bin/
    mkdir -p ~/.local/share/applications
    cp lore-tray/linux/lore.desktop ~/.local/share/applications/
    mkdir -p ~/.local/share/icons/hicolor/128x128/apps
    cp lore-tray/linux/lore.png ~/.local/share/icons/hicolor/128x128/apps/
