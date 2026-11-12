#! /bin/bash
# .github/workflows/publish.sh 0.7.0
VERSION=$1
# `cargo publish` checks a package with the crates of crates.io, where
# `netlist-macros` of this version is, and not with `vendor/`
rm -f .cargo/config.toml
sed -i "/\[workspace.package\]/,/version/ s/version = \"[^\"]*\"/version = \"$VERSION\"/" Cargo.toml
cd macros
cargo publish --allow-dirty --registry crates-io
cd ..

sleep 5

sed -i 's|netlist-macros = { path = "macros" }|netlist-macros = "'"$VERSION"'"|' Cargo.toml
cargo publish --allow-dirty --registry crates-io
sed -i 's|netlist-macros = "'"$VERSION"'"|netlist-macros = { path = "macros" }|' Cargo.toml