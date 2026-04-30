set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

# list commands also default
list:
    @just --list

sqlx:
    cargo sqlx database drop -y
    cargo sqlx database create
    cargo sqlx migrate run

run:
    cargo build
    sudo target/debug/basildisk

debian:
    sudo apt install smartmontools
