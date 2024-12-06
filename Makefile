ifneq (,$(wildcard ./.env))
    include .env
    export
endif

all: rust webext

test:
	cargo test --all

rust:
	cargo build --release

init:
	npm install -g web-ext

.PHONY: webext
webext:
	\rm -f target/webext/*
	web-ext sign --api-key $(AMO_API_KEY) --api-secret $(AMO_API_SECRET) --source-dir webext --artifacts-dir target/webext --channel unlisted 
	mv target/webext/*.xpi target/webext/keep-it-focused.xpi
	cp target/webext/keep-it-focused.xpi dist/