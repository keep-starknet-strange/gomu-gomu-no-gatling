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

For Katana, currently you need to increase the `DEFAULT_PREFUNDED_ACCOUNT_BALANCE` in constants to `0xffffffffffffffffffffffffffffffff`
and run the node with flag `--no-validate`.

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

## Resources

- Gomu Gomu is originally inspired from [Flood](https://github.com/paradigmxyz/flood)
- (Aptos load-testing tool)[https://github.com/aptos-labs/aptos-multi-region-bench]
- (Starknet RPC specs)[https://github.com/starkware-libs/starknet-specs/blob/master/api/starknet_api_openrpc.json]

## Contributors

<!-- ALL-CONTRIBUTORS-LIST:START - Do not remove or modify this section -->
<!-- prettier-ignore-start -->
<!-- markdownlint-disable -->
<table>
  <tbody>
    <tr>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/abdelhamidbakhta"><img src="https://avatars.githubusercontent.com/u/45264458?v=4?s=100" width="100px;" alt="Abdel @ StarkWare "/><br /><sub><b>Abdel @ StarkWare </b></sub></a><br /><a href="https://github.com/keep-starknet-strange/gomu-gomu-no-gatling/commits?author=abdelhamidbakhta" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/EvolveArt"><img src="https://avatars.githubusercontent.com/u/12902455?v=4?s=100" width="100px;" alt="0xevolve"/><br /><sub><b>0xevolve</b></sub></a><br /><a href="https://github.com/keep-starknet-strange/gomu-gomu-no-gatling/commits?author=EvolveArt" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://droak.sh/"><img src="https://avatars.githubusercontent.com/u/5263301?v=4?s=100" width="100px;" alt="Oak"/><br /><sub><b>Oak</b></sub></a><br /><a href="https://github.com/keep-starknet-strange/gomu-gomu-no-gatling/commits?author=d-roak" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/drspacemn"><img src="https://avatars.githubusercontent.com/u/16685321?v=4?s=100" width="100px;" alt="drspacemn"/><br /><sub><b>drspacemn</b></sub></a><br /><a href="https://github.com/keep-starknet-strange/gomu-gomu-no-gatling/commits?author=drspacemn" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/haroune-mohammedi"><img src="https://avatars.githubusercontent.com/u/118889688?v=4?s=100" width="100px;" alt="Haroune &#124; Quadratic"/><br /><sub><b>Haroune &#124; Quadratic</b></sub></a><br /><a href="https://github.com/keep-starknet-strange/gomu-gomu-no-gatling/commits?author=haroune-mohammedi" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/dbejarano820"><img src="https://avatars.githubusercontent.com/u/58019353?v=4?s=100" width="100px;" alt="Daniel Bejarano"/><br /><sub><b>Daniel Bejarano</b></sub></a><br /><a href="https://github.com/keep-starknet-strange/gomu-gomu-no-gatling/commits?author=dbejarano820" title="Code">💻</a></td>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/nicbaz"><img src="https://avatars.githubusercontent.com/u/932244?v=4?s=100" width="100px;" alt="nbz"/><br /><sub><b>nbz</b></sub></a><br /><a href="https://github.com/keep-starknet-strange/gomu-gomu-no-gatling/commits?author=nicbaz" title="Code">💻</a></td>
    </tr>
  </tbody>
</table>

<!-- markdownlint-restore -->
<!-- prettier-ignore-end -->

<!-- ALL-CONTRIBUTORS-LIST:END -->
