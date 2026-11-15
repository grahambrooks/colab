colab refactor --script "$TASK/../../../examples/packs/rust/edition-2021-to-2024.colab" --format json --detail summary --write .
colab refactor --format json --detail summary --write -e "match toml::key \"package.edition\" in \"Cargo.toml\" { set '\"2024\"' }" .
