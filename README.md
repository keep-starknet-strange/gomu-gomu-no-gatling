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
RUST_LOG=debug cargo run -- shoot -c config/default.yaml
```

## Usage

```bash
gatling --help
```

### Configuration

Gomu gomu's configuration is specified as a yaml file.
You can find example configurations under the [config](./config) folder.

> As it uses the `config` crate under the hood, the configuration could be specified as any other file type such as TOML or JSON.

The configuration is defined by the following spec

- `rpc`

  - `url`: Starknet RPC url, should be compliant with the specification

- `setup`

> `v0` and `v1` CAN'T be specified at the same time

- `erc20_contract`: ERC20 contract used to benchmark transfers

  - `v0`: Path to Cairo Zero contract artifact
  - `v1`:

    - `path`: Path to Cairo contract sierra artifact
    - `casm_path`: Path to Cairo contract casm artifact

  - `erc721_contract`: ERC721 contract used to benchmark mints
    ...

  - `account_contract`: Account contract used to send transactions
    ...

  - `fee_token_address`: Contract address of the fee token on the target chain
  - `num_accounts`: Number of accounts sending transactions

- `run`

  - `num_erc20_transfers`: Number of ERC20 `transfer` transactions
  - `num_erc721_mints`: Number of ERC721 `mint` transactions

- `report`

  - `num_blocks`: Number of last blocks to take into account in the report
  - `reports_dir`: Path to the directory where to save the reports

- `deployer`

  - `salt`: Salt used to compute deployment addresses
  - `address`: Address of the deployer account (should be pre-funded)
  - `signing_key`: Private key of the deployer signer

### Run a load test

```bash
gatling shoot -c config/default.yaml
```

## Ressources

- Gomu Gomu is originally inspired from [Flood](https://github.com/paradigmxyz/flood)
- (Aptos load-testing tool)[https://github.com/aptos-labs/aptos-multi-region-bench]
- (Starknet RPC specs)[https://github.com/starkware-libs/starknet-specs/blob/master/api/starknet_api_openrpc.json]
