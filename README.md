# CKB Bitcoin SPV Service

[![License]](#license)
[![GitHub Actions]](https://github.com/yangby-cryptape/ckb-bitcoin-spv-service/actions)

> [!WARNING]
> This repository is still in the proof-of-concept stage.

A service, which synchronizes [Bitcoin] headers to a [Bitcoin SPV on CKB]
and provides an API to generate proofs for [Bitcoin] transactions so that
they can be verified on [CKB].

[License]: https://img.shields.io/badge/License-MIT-blue.svg
[GitHub Actions]: https://github.com/yangby-cryptape/ckb-bitcoin-spv-service/workflows/CI/badge.svg

## Usage

With the command line option `-h`(alias of `--help`), help will be printed.

### JSON-RPC API Reference

- Method `getTxProof`

  Arguments:

  - `txid` (a hexadecimal string)

    A bitcoin transaction id.

    **No `0x`-prefix, as same as [the format in Bitcoin RPC APIs](https://developer.bitcoin.org/reference/rpc/gettxoutproof.html#argument-1-txids).**


  - `tx-index` (an unsigned integer)

    The index of a transaction in the block; starts from 0.

    **Service doesn't check it, just pack it into the final proof.**

  - `confirmations` (an unsigned integer)

    Represents the required acceptance of the transaction by the bitcoin network.

  Result:

  - `spv_client` ([type: `OutPoint`])

    An out point of a SPV client cell in CKB.

  - `proof` ([type: `JsonBytes`])

    The full proof of a bitcoin transaction, which could be verified with
    above SPV client in CKB.

  <details><summary>An example:</summary>


  Request:

  ```shell
  curl -X POST -H "Content-Type: application/json" \
      -d '{"jsonrpc": "2.0", "method":"getTxProof", "params": ["95231964950ce016b1333ecc1ec98bf4effc3d5d579c5c7232dddd7c2200f124", 9, 10], "id": 1}' \
      http://127.0.0.1:8888
  ```

  Output:
  ```json
  {
      "jsonrpc": "2.0",
      "result": {
          "proof": "0x3d03000014000000180000001c000000f90100000900000040bc0c00d9010000000018228d263ff4070e2fdb31b654704e99f850a4f31762155003000000000000000000f79ccd11440a9d383b1438ee60692fc7b78a4b8800faf6ec61ec5fc37a078387f998f265595a03175799484bca0700000c99f30ab6b29578c47b5e290383c73016dc51ceb8d8e4e4772b8914074d261cea95fae35702bd7842c06314b56f2fccf4f1e7a7a1e9316510eac47b8da580bd8c24f100227cdddd32725c9c575d3dfceff48bc91ecc3e33b116e00c95641923959c4eadf6c5194ce92e4af499234915ae16ef6ce925319ecdb782c15b7b79b8e0add7f560c4894d1a6373d3a0f0b2b7809de8c68881212e5804c973d810fac6ab81b4d8171567ed7b4c562cfd92f340f372d4d783ddaec72f4ae0898fa4f1a1609462a8aaa2c0038416074bef53effa98f61229bf5fba767cead3344362b1d9b440b1367c8597e5f42c52fb8b861ff7dd06ce2b6f8a1e7cb41197909a476e625c11f26490c00f5c00aa247fbe7b82cb4d42255596005d6aea0c313c425044da0155c1e3eb8cc65279a99de185af0c57f18f41ead7d93ba6dc6613f2ad338a0bc73d7c8c15eabf47c5de917babf44ce66982367ff38d9a586589fa525b72e6d8c7814386fdb76da0db6f786b020a558e7966dc94e5f75d9de94784297048c9c80103ff2e000800000041bc0c0041bc0c0083ccd2727719f195c487e165c7cebebabe280f08c4060100000000000000000042bc0c0043bc0c001e1ae1b8a976a99d3000d708b5cf41bd34a3610e18d33c7ea12090182c84fa7544bc0c0047bc0c0043bd45f3f7425544324bbdd6303adb92fe446555b5ec5548f4e481bdced123e748bc0c004fbc0c00bd692cd46c3fec36a58f2a963e8b88fb183b46848e2743dd3e599486c466bd7450bc0c005fbc0c000e8775e71c17af3afd9129e0b9cf76d1af0cc7cd49ef5e98d0c25a72b66349ac60bc0c007fbc0c001f8d8f32d7fd85ee07f5e5e9c280bab9ae9fe148bf2c03e45e786fc23024116280bc0c00bfbc0c0016b40a5455bd596f89b58207e272a8f333c03f1ec7e9dd9a1a47c20d4371eb63c0bc0c00c8bc0c006d3b9e55c56dd1fb5777bc115db1faca7a7f6e33cd9823f3a223581c740f833b",
          "spv_client": {
              "index": "0x1",
              "tx_hash": "0xcaee4eac91a43642464ee2c6582a86835f2bc10d55b15fcd22083ae9d273a1dc"
          }
      },
      "id": 1
  }
  ```

  </details>

## Related Projects

- [The Core Library of CKB Bitcoin SPV][Bitcoin SPV on CKB]

- [The CKB Bitcoin SPV Contracts](https://github.com/ckb-cell/ckb-bitcoin-spv-contracts)

## License

Licensed under [MIT License].

[Bitcoin]: https://bitcoin.org
[CKB]: https://github.com/nervosnetwork/ckb
[Bitcoin SPV on CKB]: https://github.com/ckb-cell/ckb-bitcoin-spv

[type: `OutPoint`]: https://github.com/nervosnetwork/ckb/tree/v0.114.0/rpc#type-outpoint
[type: `JsonBytes`]: https://github.com/nervosnetwork/ckb/tree/v0.114.0/rpc#type-jsonbytes

[MIT License]: LICENSE
