COLAB_KEEP_SERVER=1 colab action --at src/main.rs:3:5-6:6 --list --format json
COLAB_KEEP_SERVER=1 colab action --at src/main.rs:3:5-6:6 --kind refactor.extract --title function --write --format json --detail counts
COLAB_KEEP_SERVER=1 colab rename --symbol fun_name --in src/main.rs --to total_price --write --format json --detail counts
