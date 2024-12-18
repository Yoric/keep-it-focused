ifneq (,$(wildcard ./.env))
    include .env
    export
endif

all: daemon webext

test:
	cargo test --all

# Build the daemon
daemon:
	cargo build --release
	cp target/release/keep-it-focused dist/

init:
	npm install -g web-ext

# Build and sign the web extension
.PHONY: webext
webext:
	\rm -f target/webext/*
	web-ext sign --api-key $(AMO_API_KEY) --api-secret $(AMO_API_SECRET) --source-dir webext --artifacts-dir target/webext --channel unlisted 
	mv target/webext/*.xpi target/webext/keep-it-focused.xpi
	cp target/webext/keep-it-focused.xpi dist/

install-daemon:
	cargo build --release
	sudo systemctl stop keep-it-focused
	sudo target/release/keep-it-focused setup
