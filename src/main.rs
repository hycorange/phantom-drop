//! `phantom` — command-line client for Phantom Drop.

use std::{
    fs,
    io::Write,
    num::NonZeroU64,
    path::{Path as FsPath, PathBuf},
    str::FromStr,
};

use anyhow::{Context, anyhow, bail, ensure};
use clap::{Parser, Subcommand};
use ootle_rs::{
    Address, Epoch, Network, default_indexer_url,
    keys::OotleSecretKey,
    provider::{IndexerProvider, ProviderBuilder, WalletProvider},
    wallet::OotleWallet,
};
use phantom_drop::{
    EnvelopePlan, EnvelopeStatus, Funding, MasterSeed, Path, PhantomLink, Riddle,
    keys::public_key_bytes,
    ops::{build_seal, build_spend, inspect, wallet_for},
    scan::scan_owned,
};
use serde::{Deserialize, Serialize};
use tari_crypto::{ristretto::RistrettoSecretKey, tari_utilities::hex::Hex};
use tari_ootle_transaction::Transaction;

const MICRO: u64 = 1_000_000;

#[derive(Parser)]
#[command(
    name = "phantom",
    version,
    about = "👻 Phantom Drop — haunted claim links for stealth TARI on Tari Ootle"
)]
struct Cli {
    /// Ootle network: esmeralda (testnet) or localnet.
    #[arg(
        long,
        global = true,
        env = "PHANTOM_NETWORK",
        default_value = "esmeralda"
    )]
    network: String,
    /// Indexer URL (defaults to the network's public indexer).
    #[arg(long, global = true, env = "PHANTOM_INDEXER")]
    indexer: Option<String>,
    /// Where the sender's master secret and envelope journal live.
    #[arg(long, global = true, env = "PHANTOM_HOME")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create your sender master secret (once per network).
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Show your sender wallet address (fund it, or burn-claim into it).
    Address,
    /// Network epoch and indexer health.
    Status,
    /// Scan the chain for stealth UTXOs owned by your sender wallet.
    Balance {
        #[arg(long, default_value_t = 0)]
        from_epoch: u64,
    },
    /// Seal one or more envelopes and print their phantom links.
    Seal {
        /// TARI per envelope, e.g. 2 or 0.5
        #[arg(long)]
        amount: String,
        #[arg(long, default_value_t = 1)]
        count: u32,
        #[arg(long, default_value = "👻 Happy Halloween!")]
        message: String,
        /// Riddle question; the answer becomes part of the claim key.
        #[arg(long, requires = "answer")]
        riddle: Option<String>,
        #[arg(long, requires = "riddle")]
        answer: Option<String>,
        /// Epochs until the link dies and only you can reclaim.
        #[arg(long, default_value_t = 200)]
        expires_in: u64,
        /// faucet | account | utxos
        #[arg(long, default_value = "utxos")]
        fund: String,
        /// Scan start epoch when --fund utxos.
        #[arg(long, default_value_t = 0)]
        from_epoch: u64,
        /// Fee in µT for the seal transaction.
        #[arg(long, default_value_t = 50_000)]
        fee: u64,
        /// Build and sign but do not submit.
        #[arg(long)]
        dry_run: bool,
    },
    /// Inspect a link: amount, message, riddle, expiry — readable only by link holders.
    Peek {
        link: String,
        #[arg(long)]
        answer: Option<String>,
    },
    /// Claim a link into an Ootle address. The fee comes out of the envelope: no funds needed.
    Open {
        link: String,
        /// Recipient Ootle address (otl_...)
        #[arg(long, conflicts_with = "new_wallet")]
        to: Option<String>,
        /// Create a fresh wallet key file here and claim into it.
        #[arg(long)]
        new_wallet: Option<PathBuf>,
        #[arg(long)]
        answer: Option<String>,
        #[arg(long, default_value_t = 50_000)]
        fee: u64,
        #[arg(long)]
        dry_run: bool,
    },
    /// Reclaim expired, unopened envelopes back to your sender wallet via the hidden refund branch.
    Reclaim {
        /// Journal index to reclaim; omit to reclaim every expired unopened envelope.
        #[arg(long)]
        index: Option<u64>,
        #[arg(long, default_value_t = 50_000)]
        fee: u64,
        #[arg(long)]
        dry_run: bool,
    },
    /// List the envelopes you sealed and their live status.
    List,
}

#[derive(Serialize, Deserialize, Clone)]
struct JournalEntry {
    index: u64,
    link: String,
    amount_micro: u64,
    expiry_epoch: u64,
    claim_key: String,
    riddle: Option<String>,
    created: String,
    seal_tx: Option<String>,
}

struct Store {
    dir: PathBuf,
    network: Network,
}

impl Store {
    fn new(home: Option<PathBuf>, network: Network) -> anyhow::Result<Self> {
        let base = match home {
            Some(h) => h,
            None => dirs_home().join(".phantom"),
        };
        let dir = base.join(network.to_string().to_lowercase());
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self { dir, network })
    }

    fn master_path(&self) -> PathBuf {
        self.dir.join("master.phantom-secret")
    }

    fn journal_path(&self) -> PathBuf {
        self.dir.join("journal.json")
    }

    fn master(&self) -> anyhow::Result<MasterSeed> {
        let hex_str = fs::read_to_string(self.master_path()).with_context(|| {
            format!(
                "no master secret for {} — run `phantom init` first",
                self.network
            )
        })?;
        let bytes: [u8; 32] = hex::decode(hex_str.trim())?
            .try_into()
            .map_err(|_| anyhow!("corrupt master secret"))?;
        Ok(MasterSeed::from_bytes(bytes))
    }

    fn journal(&self) -> anyhow::Result<Vec<JournalEntry>> {
        match fs::read_to_string(self.journal_path()) {
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    fn save_journal(&self, entries: &[JournalEntry]) -> anyhow::Result<()> {
        write_private(
            &self.journal_path(),
            serde_json::to_string_pretty(entries)?.as_bytes(),
        )
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn write_private(path: &FsPath, contents: &[u8]) -> anyhow::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    f.write_all(contents)?;
    Ok(())
}

fn parse_network(s: &str) -> anyhow::Result<Network> {
    match s.to_lowercase().as_str() {
        "esmeralda" | "esme" | "testnet" => Ok(Network::Esmeralda),
        "localnet" | "local" => Ok(Network::LocalNet),
        other => Network::from_str(other).map_err(|_| anyhow!("unknown network {other}")),
    }
}

fn parse_tari(s: &str) -> anyhow::Result<u64> {
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    ensure!(frac.len() <= 6, "at most 6 decimals (1 µT)");
    let whole: u64 = if whole.is_empty() { 0 } else { whole.parse()? };
    let frac: u64 = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<6}").parse()?
    };
    Ok(whole * MICRO + frac)
}

fn fmt_tari(micro: u64) -> String {
    format!("{}.{:06} TARI", micro / MICRO, micro % MICRO)
}

async fn connect(
    cli_indexer: &Option<String>,
    network: Network,
    wallet: OotleWallet,
) -> anyhow::Result<IndexerProvider<OotleWallet>> {
    let url = cli_indexer
        .clone()
        .unwrap_or_else(|| default_indexer_url(network).to_string());
    ProviderBuilder::new()
        .with_network(network)
        .wallet(wallet)
        .connect(&url)
        .await
        .with_context(|| format!("connecting to indexer {url}"))
}

async fn submit(
    provider: &mut IndexerProvider<OotleWallet>,
    tx: Transaction,
    dry_run: bool,
) -> anyhow::Result<Option<String>> {
    let id = tx.calculate_id().to_string();
    if dry_run {
        println!("🧪 dry run — signed transaction {id} not submitted");
        return Ok(None);
    }
    println!("📨 submitting {id} …");
    let pending = provider.send_transaction(tx).await?;
    let outcome = pending.watch().await?;
    println!("🏁 finalized: {outcome:?}");
    let receipt = pending.get_receipt().await?;
    ensure!(
        format!("{:?}", receipt.outcome)
            .to_lowercase()
            .contains("commit"),
        "transaction was not committed: {:?}",
        receipt.outcome
    );
    println!(
        "   fees paid: {}",
        fmt_tari(receipt.fee_receipt.total_fees_paid())
    );
    Ok(Some(id))
}

fn print_link_card(link: &PhantomLink, amount: u64, riddle: Option<&str>) {
    println!("────────────────────────────────────────────────────────────");
    println!(
        "👻 {} · expires at epoch {}",
        fmt_tari(amount),
        link.expiry_epoch
    );
    if let Some(q) = riddle {
        println!("🎃 riddle: {q}");
    }
    println!("{link}");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let network = parse_network(&cli.network)?;
    let store = Store::new(cli.home.clone(), network)?;

    match cli.cmd {
        Cmd::Init { force } => {
            if store.master_path().exists() && !force {
                bail!(
                    "{} already exists (use --force to overwrite and lose access to old envelopes)",
                    store.master_path().display()
                );
            }
            let master = MasterSeed::random();
            write_private(
                &store.master_path(),
                hex::encode(master.as_bytes()).as_bytes(),
            )?;
            println!(
                "🔐 master secret written to {} (back it up!)",
                store.master_path().display()
            );
            println!(
                "💰 sender wallet address: {}",
                master.wallet_key(network).to_address()
            );
        }
        Cmd::Address => {
            println!("{}", store.master()?.wallet_key(network).to_address());
        }
        Cmd::Status => {
            let provider = connect(
                &cli.indexer,
                network,
                wallet_for(OotleSecretKey::random(network)),
            )
            .await?;
            println!("network: {network}");
            println!("indexer network: {}", provider.get_network().await?);
            println!("current epoch: {}", provider.get_epoch().await?.as_u64());
        }
        Cmd::Balance { from_epoch } => {
            let key = store.master()?.wallet_key(network);
            let provider = connect(&cli.indexer, network, wallet_for(key.clone())).await?;
            let owned = scan_owned(&provider, &key, Epoch(from_epoch)).await?;
            let total: u64 = owned.iter().map(|o| o.value).sum();
            for o in &owned {
                println!("  {}  {}", o.commitment, fmt_tari(o.value));
            }
            println!(
                "total: {} in {} stealth UTXO(s)",
                fmt_tari(total),
                owned.len()
            );
        }
        Cmd::Seal {
            amount,
            count,
            message,
            riddle,
            answer,
            expires_in,
            fund,
            from_epoch,
            fee,
            dry_run,
        } => {
            let master = store.master()?;
            let sender = master.wallet_key(network);
            let amount =
                NonZeroU64::new(parse_tari(&amount)?).context("amount must be positive")?;
            ensure!((1..=64).contains(&count), "count must be 1..=64");
            let mut provider = connect(&cli.indexer, network, wallet_for(sender.clone())).await?;
            let epoch = provider.get_epoch().await?.as_u64();
            let expiry = epoch + expires_in;
            let riddle = riddle.map(|question| Riddle {
                question,
                answer: answer.expect("clap enforces --answer"),
            });

            let mut journal = store.journal()?;
            let first = journal.iter().map(|e| e.index + 1).max().unwrap_or(0);
            let plans = (0..count as u64)
                .map(|i| {
                    EnvelopePlan::new(
                        network,
                        &master,
                        first + i,
                        amount,
                        expiry,
                        &message,
                        riddle.clone(),
                    )
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let needed = amount.get() * count as u64 + fee;

            let funding = match fund.as_str() {
                "faucet" => Funding::Faucet,
                "account" => Funding::Account,
                "utxos" => {
                    let mut owned = scan_owned(&provider, &sender, Epoch(from_epoch)).await?;
                    owned.sort_by_key(|o| std::cmp::Reverse(o.value));
                    let mut picked = vec![];
                    let mut sum = 0;
                    for o in owned {
                        if sum >= needed {
                            break;
                        }
                        sum += o.value;
                        picked.push(o.commitment);
                    }
                    ensure!(
                        sum >= needed,
                        "your sender wallet holds {} in stealth UTXOs; {} needed. Fund {} first.",
                        fmt_tari(sum),
                        fmt_tari(needed),
                        sender.to_address()
                    );
                    Funding::Utxos(picked)
                }
                other => bail!("unknown funding source {other}"),
            };

            let max_epoch = Epoch(epoch + 10);
            let batch = build_seal(&provider, &sender, funding, &plans, fee, max_epoch).await?;
            let tx_id = batch.transaction.calculate_id().to_string();
            // Journal first: if the process dies after submission, the links are not lost.
            for (plan, link) in plans.iter().zip(&batch.links) {
                journal.push(JournalEntry {
                    index: plan.index,
                    link: link.to_string(),
                    amount_micro: amount.get(),
                    expiry_epoch: expiry,
                    claim_key: hex::encode(plan.claim_key.as_ref()),
                    riddle: plan.riddle.as_ref().map(|r| r.question.clone()),
                    created: chrono::Utc::now().to_rfc3339(),
                    seal_tx: (!dry_run).then(|| tx_id.clone()),
                });
            }
            let before = store.journal()?;
            if !dry_run {
                store.save_journal(&journal)?;
            }
            if let Err(e) = submit(&mut provider, batch.transaction, dry_run).await {
                // A definite rejection means the envelopes never came to exist; anything else (a timeout,
                // a dropped connection) might still commit, so those links stay in the journal.
                if e.to_string().contains("rejected") || e.to_string().contains("not committed") {
                    store.save_journal(&before)?;
                }
                return Err(e);
            }
            for (plan, link) in plans.iter().zip(&batch.links) {
                print_link_card(
                    link,
                    amount.get(),
                    plan.riddle.as_ref().map(|r| r.question.as_str()),
                );
            }
            println!("────────────────────────────────────────────────────────────");
            println!(
                "Share a link privately (or publicly, if it is riddle-locked). Whoever holds it can `phantom open` it."
            );
        }
        Cmd::Peek { link, answer } => {
            let link: PhantomLink = link.parse()?;
            let provider = connect(
                &cli.indexer,
                link.network,
                wallet_for(link.seed.ghost_key(link.network)),
            )
            .await?;
            let epoch = provider.get_epoch().await?.as_u64();
            match inspect(&provider, &link, answer.as_deref()).await? {
                EnvelopeStatus::Gone => {
                    println!("🕳️  this envelope has already been opened or reclaimed")
                }
                EnvelopeStatus::Unspent(info) => {
                    println!("👻 holds {}", fmt_tari(info.value));
                    println!("💌 {}", info.message);
                    if let Some(q) = &info.riddle {
                        println!("🎃 riddle: {q}");
                    }
                    match info.answer_ok {
                        Some(true) if link.riddle => println!("✅ that answer opens it"),
                        Some(false) => println!("❌ that answer does not open it"),
                        None => println!("🔒 riddle-locked: pass --answer to check a guess"),
                        _ => {}
                    }
                    if epoch < info.expiry_epoch {
                        println!("⏳ open before epoch {} (now {epoch})", info.expiry_epoch);
                    } else {
                        println!(
                            "💀 expired at epoch {} — only the sender can reclaim it now",
                            info.expiry_epoch
                        );
                    }
                }
            }
        }
        Cmd::Open {
            link,
            to,
            new_wallet,
            answer,
            fee,
            dry_run,
        } => {
            let link: PhantomLink = link.parse()?;
            ensure!(
                !link.riddle || answer.is_some(),
                "this envelope is riddle-locked: pass --answer"
            );
            let recipient: Address = match (to, new_wallet) {
                (Some(addr), _) => addr.parse().map_err(|e| anyhow!("bad address: {e:?}"))?,
                (None, Some(path)) => {
                    let key = OotleSecretKey::random(link.network);
                    let json = serde_json::json!({
                        "network": link.network.to_string(),
                        "address": key.to_address().to_string(),
                        "account_secret": key.account_secret().to_hex(),
                        "view_secret": key.view_only_secret().to_hex(),
                    });
                    write_private(&path, serde_json::to_string_pretty(&json)?.as_bytes())?;
                    println!("🆕 new wallet key written to {}", path.display());
                    key.to_address()
                }
                (None, None) => bail!("pass --to <otl address> or --new-wallet <file>"),
            };
            let mut provider = connect(
                &cli.indexer,
                link.network,
                wallet_for(link.seed.ghost_key(link.network)),
            )
            .await?;
            let epoch = provider.get_epoch().await?.as_u64();
            ensure!(
                epoch < link.expiry_epoch,
                "this link expired at epoch {} (now {epoch})",
                link.expiry_epoch
            );
            let claim_secret = link.seed.claim_secret(answer.as_deref());
            let (tx, payout) = build_spend(
                &provider,
                &link,
                Path::Claim,
                &claim_secret,
                public_key_bytes(&claim_secret),
                &recipient,
                fee,
                Epoch(epoch + 10),
            )
            .await?;
            submit(&mut provider, tx, dry_run).await?;
            println!("🎁 {} → {recipient}", fmt_tari(payout));
        }
        Cmd::Reclaim {
            index,
            fee,
            dry_run,
        } => {
            let master = store.master()?;
            let sender = master.wallet_key(network);
            let journal = store.journal()?;
            let mut provider = connect(&cli.indexer, network, wallet_for(sender.clone())).await?;
            let epoch = provider.get_epoch().await?.as_u64();
            let mut done = 0;
            for entry in journal
                .iter()
                .filter(|e| index.is_none_or(|i| i == e.index))
            {
                let link: PhantomLink = entry.link.parse()?;
                *provider.wallet_mut() = wallet_for(link.seed.ghost_key(network));
                if matches!(inspect(&provider, &link, None).await?, EnvelopeStatus::Gone) {
                    continue;
                }
                if epoch < link.expiry_epoch {
                    println!(
                        "⏳ #{} not expired yet (epoch {} < {})",
                        entry.index, epoch, link.expiry_epoch
                    );
                    continue;
                }
                let claim_key: [u8; 32] = hex::decode(&entry.claim_key)?
                    .try_into()
                    .map_err(|_| anyhow!("bad claim key"))?;
                let refund_secret: RistrettoSecretKey = master.refund_secret(&link.seed);
                let (tx, payout) = build_spend(
                    &provider,
                    &link,
                    Path::Refund,
                    &refund_secret,
                    claim_key.into(),
                    &sender.to_address(),
                    fee,
                    Epoch(epoch + 10),
                )
                .await?;
                submit(&mut provider, tx, dry_run).await?;
                println!("↩️  #{} reclaimed {}", entry.index, fmt_tari(payout));
                done += 1;
            }
            println!("{done} envelope(s) reclaimed");
        }
        Cmd::List => {
            let journal = store.journal()?;
            let provider = connect(
                &cli.indexer,
                network,
                wallet_for(store.master()?.wallet_key(network)),
            )
            .await?;
            let epoch = provider.get_epoch().await?.as_u64();
            let mut provider = provider;
            for e in &journal {
                let link: PhantomLink = e.link.parse()?;
                *provider.wallet_mut() = wallet_for(link.seed.ghost_key(network));
                let state = match inspect(&provider, &link, None).await {
                    Ok(EnvelopeStatus::Gone) => "opened/reclaimed".to_string(),
                    Ok(EnvelopeStatus::Unspent(_)) if epoch >= e.expiry_epoch => {
                        "EXPIRED — reclaimable".to_string()
                    }
                    Ok(EnvelopeStatus::Unspent(_)) => {
                        format!("waiting (expires {})", e.expiry_epoch)
                    }
                    Err(err) => format!("unknown ({err})"),
                };
                println!(
                    "#{:<3} {:>18}  {}{}",
                    e.index,
                    fmt_tari(e.amount_micro),
                    state,
                    if e.riddle.is_some() { "  🎃" } else { "" }
                );
            }
        }
    }
    Ok(())
}
