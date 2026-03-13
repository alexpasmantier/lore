# Build and install binaries to ~/.local/bin
install:
    cargo build --release -p lore-mcp -p lore-daemon && cp target/release/lore-daemon target/release/lore-mcp ~/.local/bin/
