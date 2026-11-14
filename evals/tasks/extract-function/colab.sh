colab action --at src/main.rs:3:5-6:6 --list --format json
colab action --at src/main.rs:3:5-6:6 --kind refactor.extract --title function --write --format json --detail counts
colab rename --symbol fun_name --in src/main.rs --to total_price --write --format json --detail counts
