<!-- markdownlint-disable -->
<div align="center">
  <h1> Gomu Gomu no Gatling </h1>
  <img src="./docs/images/gomu-gomu-no-bg.png" width="256">
</div>
<br />
<!-- markdownlint-restore -->

[![GitHub Workflow Status](https://github.com/keep-starknet-strange/gomu-gomu-no-gatling/actions/workflows/push.yml/badge.svg)](https://github.com/keep-starknet-strange/gomu-gomu-no-gatling/actions/workflows/push.yml)
[![Project license](https://img.shields.io/github/license/keep-starknet-strange/gomu-gomu-no-gatling.svg?style=flat-square)](LICENSE)
[![Pull Requests welcome](https://img.shields.io/badge/PRs-welcome-ff69b4.svg?style=flat-square)](https://github.com/keep-starknet-strange/gomu-gomu-no-gatling/issues?q=is%3Aissue+is%3Aopen+label%3A%22help+wanted%22)
[![Rust docs](https://docs.rs/anthropic/badge.svg)](https://docs.rs/gatling)
[![Rust crate](https://img.shields.io/crates/v/galing.svg)](https://crates.io/crates/gatling)

Blazing fast tool to benchmark Starknet sequencers 🦀.

## Installation

### From source

```bash
git clone https://github.com/keep-starknet-strange/gomu-gomu-no-gatling
cd gomu-gomu-no-gatling
cargo install --path .
```

### From crates.io

```bash
cargo install --locked gatling
```

### Run debug

```bash
RUST_LOG=info cargo run -- shoot -c config/rinnegan.yaml
```

## Usage

```bash
gatling --help
```

### Configuration

> **TODO**: Add configuration options.

### Run a load test

```bash
gatling shoot -c config/rinnegan.yaml
```
