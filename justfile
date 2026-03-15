# Build and install all binaries to ~/.local/bin
install:
    cargo build --release -p lore-mcp -p lore-daemon -p lore-tray -p lore-server -p lore-explorer
    cp target/release/lore target/release/lore-{mcp,tray,server,explorer} ~/.local/bin/

# Build macOS .app bundle at target/Lore.app
bundle-macos:
    cargo build --release -p lore-tray -p lore-daemon
    rm -rf target/Lore.app
    mkdir -p target/Lore.app/Contents/MacOS
    mkdir -p target/Lore.app/Contents/Resources
    cp lore-tray/macos/Info.plist target/Lore.app/Contents/
    cp lore-tray/macos/AppIcon.icns target/Lore.app/Contents/Resources/
    cp target/release/lore-tray target/Lore.app/Contents/MacOS/
    cp target/release/lore target/Lore.app/Contents/MacOS/
    @echo "Bundle created at target/Lore.app"

# Install on Linux with .desktop entry and icon
install-linux:
    cargo build --release -p lore-mcp -p lore-daemon -p lore-tray -p lore-server -p lore-explorer
    mkdir -p ~/.local/bin
    cp target/release/lore target/release/lore-{mcp,tray,server,explorer} ~/.local/bin/
    mkdir -p ~/.local/share/applications
    cp lore-tray/linux/lore.desktop ~/.local/share/applications/
    mkdir -p ~/.local/share/icons/hicolor/128x128/apps
    cp lore-tray/linux/lore.png ~/.local/share/icons/hicolor/128x128/apps/

# Build Docker image for lore-server
docker-build:
    docker build -t lore-server .

# Start a local lore-server in Docker for testing remote mode
remote-test-up: docker-build
    -docker rm -f lore-test-server 2>/dev/null
    docker run -d --name lore-test-server -p 8080:8080 \
        -v lore-test-data:/data \
        -e ANTHROPIC_API_KEY \
        lore-server
    @echo ""
    @echo "Server running at http://localhost:8080"
    @echo "  MCP endpoint: http://localhost:8080/mcp"
    @echo "  Push endpoint: http://localhost:8080/push"
    @echo "  Status: http://localhost:8080/status"
    @echo ""
    @if [ -n "$$ANTHROPIC_API_KEY" ]; then echo "Consolidation: enabled"; else echo "Consolidation: DISABLED (set ANTHROPIC_API_KEY)"; fi

# Stop the local test server
remote-test-down:
    -docker stop lore-test-server
    -docker rm lore-test-server
    @echo "Server stopped."

# Show test server status and staged turn counts
remote-test-status:
    @curl -s http://localhost:8080/status | python3 -m json.tool

# Tail test server logs
remote-test-logs:
    docker logs -f lore-test-server
