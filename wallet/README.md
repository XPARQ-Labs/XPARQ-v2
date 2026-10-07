# XPARQ Wallet

`wallet/` contains the reusable `wallet` library and the `wallet`
executable. It never opens node storage and communicates through HTTP RPC.

```bash
cargo build --release --locked -p wallet
./target/release/wallet
./target/release/wallet --help
```

The interactive menu supports wallet creation and restoration, balances,
canonical history, UTXO tracking and consolidation, XPQ sends, Program assets,
block exploration, and bytecode deployment.

## Security

The wallet file contains recovery and private signing material. It is checked
for internal consistency when loaded, created atomically, and given owner-only
permissions on Unix. Back up the mnemonic offline and never commit, upload, or
share the wallet file. A miner needs only the public payout address. Keep plain
HTTP RPC on loopback or a trusted private network.

## XPQ transactions

Signed transactions are submitted automatically to `/transaction`. Use
`--offline` to print signed canonical transaction bytes without submitting them.
RPC is still needed when selecting inputs or calculating fees from node state.

```bash
./target/release/wallet sign-spend --to ADDRESS --amount 1 --rpc 127.0.0.1:6666
./target/release/wallet consolidate --wallet wallet.json --rpc 127.0.0.1:6666
```

Without explicit inputs, the wallet selects available `/account/{address}`
UTXOs and creates change. Consolidation merges the selected XPQ UTXOs into one
self-owned output. Consensus validates it as an ordinary transaction, so its
canonical bytes still incur archival burn and a miner fee.

The miner fee is node policy. The protocol burn separately covers canonical
transaction history and positive net state growth. Consumed Coin UTXOs offset
new Coin UTXOs for state growth but do not erase historical transaction bytes.

## Extension asset program

For integration details, see [ProgramCall documentation](../docs/PROGRAM_CALL.md).

The asset application runs in extension; canonical records and shares belong to
kernel `LedgerState.extensions.assets`. This is separate from the deployed-code
registry. Use `program-*` commands for assets. Amounts use
8 decimal places. XPQ fee inputs, change, miner fee and exact protocol burn are
selected automatically. Commands need a running node for balances and quotes.

The examples below assume the `wallet` executable is on PATH; otherwise use
`./target/release/wallet`. Set `--wallet` and `--rpc` as needed on every command.

```sh
wallet program-register --name Gold --max-supply 100 --initial-mint 40 --wallet wallet.json --rpc 127.0.0.1:6666
wallet program-mint --asset CONTRACT --to ADDRESS --amount 20
wallet program-transfer --asset CONTRACT --to ADDRESS --amount 15
wallet program-burn --asset CONTRACT --amount 5
wallet program-consolidate --asset CONTRACT
wallet program-info --asset CONTRACT
wallet program-balance --asset CONTRACT --address ADDRESS
```

`--fixed-supply` disables further minting at registration. `--offline` prints
canonical signed transaction hex without submitting it; RPC is still needed to
select XPQ/share inputs and quote state growth. Transfer/burn consume at most
256 shares per call. Consolidation merges up to 256 shares per invocation and
requires at least two shares. Wait for pending asset operations to confirm
before submitting a dependent operation.

Interactive mode has a **Program Assets** menu. Normal balance and history show
extension holdings and operations; asset-only recipients also receive history
entries even when they receive no XPQ.

RPC reads: `/program/asset/{asset}` and
`/program/asset/{asset}/balance/{address}`. `/account` and `/balance` expose a
separate `program_assets` array; `/explorer/address` contains aggregate balances
without share lists. `/explorer/transaction` decodes Program asset instructions.
`POST /program/quote` reads a signed canonical Program transaction and returns
extension state growth in bytes without admitting or applying the transaction.
Its quote reflects canonical state; final submission revalidates against mempool
state and can reject a stale quote.

## Deployed programs

`program-register` creates an asset; `program-deploy` deploys validated XPVM
bytecode. Choose an unused nonce for the deploying owner.

```sh
./target/release/wallet program-deploy --code program.xpvm --nonce 1 --wallet wallet.json --rpc 127.0.0.1:6666
```

The wallet obtains a deploy quote, selects XPQ funding, signs and submits to
`/program/deploy`. `--offline` prints signed bytes instead of submitting; RPC is
still required for funding and the quote. See [XPVM](../docs/XPVM.md) for the
bytecode format. To execute a deployed program after deployment is confirmed:

```sh
./target/release/wallet program-call --program-id PROGRAM_ID --wallet wallet.json --rpc 127.0.0.1:6666
```

The ID must contain exactly 64 hexadecimal characters. The command signs a VM
call with opcode 0 and the program ID as payload, quotes state growth, selects
funding, and calculates burn and miner fees. `--offline` prints signed bytes
and still requires RPC. It returns a transaction hash on submission; wait for
confirmation before sending another call using the same wallet inputs.
See the [counter example](../examples/counter/README.md).

### Integration verification

```sh
cargo build -p node -p wallet --bins
cargo test -p wallet --test program_e2e -- --ignored --test-threads=1
```

These ignored tests require an explicit run. Both passed on 3 October 2026:
XPQ spend/consolidation and the asset lifecycle. They use real wallet/node binaries and temporary redb storage for register,
mint, transfer, burn, consolidation, balances, recipient history and restart.

## Contract balances and funding

`sign-spend`, `program-transfer`, and `program-mint` accept a deployed contract
recipient as `--to program:PROGRAM_ID`. Addresses retain their usual syntax.
`program-account --program-id PROGRAM_ID` displays the contract's coin balance
and live coin/asset shares. `program-call` executes its code and pays costs from
the calling wallet. See [the fixed payout contract](../examples/vault/README.md)
for an example that receives and sends both values.


XPVM v3 programs can also register and mint native assets with the deployed program
as mint authority. `program-call` quotes and pays the resulting state growth and
fuel from the caller's wallet. See the [asset issuer example](../examples/asset_issuer/README.md).
