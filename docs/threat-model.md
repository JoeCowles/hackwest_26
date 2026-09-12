# Threat model and anomaly detection

*Status: Draft v0.1*

*Scope: What malicious activity a Mac mini cluster with shared NFS storage realistically faces, which of it is visible in filesystem telemetry, and how to build a detector that earns its alerts.*

Addresses the challenge requirement to "provide reporting mechanisms to alert
administrators of nefarious users and/or capacity concerns," and fills in
Server Spec §4.8 (*Security and anomalous behavior*).

**Framing principle, carried over from Server Spec §4.8:** alerts describe
*observable behavior*, never a verdict about a person. "This account deleted
40,000 files in 90 seconds, 200x its 30-day baseline" is a finding. "Nefarious
user" is a libel risk and tells an administrator nothing actionable.

---

## 1. What macOS actually exposes

Verified on an M-series Mac running macOS 26 (Darwin 25.6.0), SIP enabled. This
table is the binding constraint on everything downstream — design the detector
around what is actually collectable, not around what the literature assumes.

| Source | Gives | Cost | Verdict |
| --- | --- | --- | --- |
| `nfsstat -s -f JSON` | 21 per-operation server RPC counters (Read, Write, Remove, Rename, Setattr, Readdir …), faults, cache stats | none — no root, no entitlement | **Primary signal.** Already wired into `nfs-lab` daemon |
| `nfsstat -c -f JSON` | Client-side counters, cache hit/miss, RPC timeouts/retries | none | **Primary.** Also wired in |
| `statfs` / `df` | Capacity, used, available per mount | none | **Primary.** Wired in |
| `quota` / `repquota` | Per-user quota limits and usage | root | **Use.** Enables per-user capacity attribution |
| `tmutil listlocalsnapshots` | APFS snapshot inventory | none | **Use.** Snapshot deletion is a ransomware precursor |
| `/dev/fsevents` | Raw file create/delete/rename/modify events | root | **Stretch.** Rich, but *no process or user attribution* |
| `fs_usage` | Per-process file I/O with pathnames | root; SIP hides protected processes | **Stretch.** Best process attribution available without entitlement |
| `log show` / `log stream` | Authentication failures, mount events, nfsd messages | none | **Use** for auth-failure counting |
| `dtrace` / `opensnoop` | Syscall-level tracing | root; SIP blocks protected binaries | Demo only — too restricted to rely on |
| **Endpoint Security framework** | Everything: process, file, mount, auth, with full attribution | **Apple-granted entitlement + SIP disabled** | **Not feasible.** Entitlement requires an application to Apple with a multi-week turnaround. Document as future work, do not plan around it |

The Endpoint Security exclusion is the single most important finding here. Most
published macOS file-monitoring work assumes ES. We cannot use it, and a design
that quietly depends on it will not run on a judge's machine.

### 1.1 The attribution problem

`nfsstat -s` counters are **aggregate per server, not per client or per user.**
This is the central limitation, and it interacts badly with NFS's auth model:

- **AUTH_SYS identity is self-asserted.** An NFSv3/AUTH_SYS client sends its own
  UID/GID in every RPC and the server trusts them. Any user with root on any
  permitted client can impersonate any UID on the export. Host ACLs
  (`-network`/`-mask`) are IP-based and therefore spoofable. Tools to do this are
  off-the-shelf ([NfSpy](https://github.com/bonsaiviking/NfSpy)).
- **`-mapall` defeats impersonation but destroys attribution.** `nfs-lab` defaults
  to `-mapall`, which squashes every client user to one identity. That is the
  right *security* default, and it makes per-user quotas and per-user baselines
  impossible, because every request now arrives as the same principal.

**These two goals are in direct conflict and the team has to pick.** Three ways out,
in ascending order of cost:

1. **Per-tenant exports.** One export and one directory per node or per team, each
   with its own `-mapall` identity. Attribution becomes *structural* — derived from
   which export was touched — instead of inferred from a spoofable UID. Cheap,
   robust, and recommended for the demo.
2. **`-maproot` only**, letting other UIDs pass through. Restores per-user
   attribution, accepts that a root-on-client attacker can forge any non-root UID.
3. **`sec=krb5`.** Real cryptographic identity, kills UID spoofing outright. Correct
   answer for production; too much setup for this weekend.

Recommendation: **(1) for the demo, (3) named explicitly as future work.** Say out
loud in the writeup that AUTH_SYS attribution is advisory, not evidentiary —
judges who know NFS will ask.

---

## 2. Threat catalog

### 2.1 NFS protocol and configuration

| Threat | Detection signal |
| --- | --- |
| UID spoofing via AUTH_SYS | Not detectable from counters alone. Mitigate by config; flag `sec=sys` as a standing risk in the dashboard |
| Missing `-maproot`/`-mapall` → remote root becomes local root | Config audit: read back effective export options, alert on their absence |
| `-alldirs` allowing arbitrary subdirectory mounts | Config audit |
| Export reachable outside the intended subnet | Compare effective `-network`/`-mask` against expected; check `2049/tcp` exposure |
| Unencrypted transit (v3/AUTH_SYS) — file contents readable on the wire | Not detectable; a documented accepted risk on a trusted VLAN |
| Export enumeration / recon | `showmount` probes; Lookup + Readdir + Getattr spike with near-zero Read/Write |
| Mount-option drift (e.g. remount rw) | Inventory diff between samples — Server Spec §4.2 already collects effective options |

### 2.2 Filesystem-observable behavior

This is what the detector actually watches. Each row is a distinguishable shape
in the per-operation counter deltas.

| Behavior | Counter signature | Capacity signature |
| --- | --- | --- |
| **Ransomware (encrypt in place)** | Rename ↑↑ **and** Write ↑ **and** Read ↑ together; Remove ↑ | used_kb roughly **flat** — the giveaway |
| **Mass deletion / sabotage** | Remove ↑↑, Rmdir ↑ | used_kb ↓ steeply |
| **Bulk exfiltration** | Read ↑↑ sustained, Write ≈ 0; Readdir/Lookup burst first (traversal) | used_kb flat |
| **Low-and-slow exfiltration** | Read modestly above baseline, sustained for hours/days | flat — needs a long baseline window to catch |
| **Storage abuse / quota exhaustion** | Write ↑ sustained, Read low | used_kb ↑ monotonic toward quota |
| **Recon / directory traversal** | Lookup + Readdir + Getattr ↑, Read/Write ≈ 0; Lookup cache-miss ratio high | flat |
| **Model-weight theft** (AI-cluster specific) | Read ↑↑ in few very large sequential transfers; high bytes-per-op | flat |

Two discriminators do most of the work:

- **Rename volume separates ransomware from ordinary bulk work.** Legitimate
  archivers and backup jobs produce high-entropy writes and heavy reads, but they
  do not mass-rename and mass-delete. This is the most robust behavioral
  distinction in the literature, and it is visible in counters we already collect.
- **Capacity delta separates encryption from deletion from hoarding.** Encrypting
  in place leaves used bytes flat while churning operations; deletion drops it;
  abuse raises it.

### 2.3 macOS and cluster-specific

| Threat | Detection signal |
| --- | --- |
| **APFS snapshot destruction** | `tmutil listlocalsnapshots` count drops. Near-universal ransomware precursor — deleting recovery points before encrypting. High-value, trivially cheap signal |
| LaunchDaemon/LaunchAgent persistence | Inventory `/Library/LaunchDaemons`, alert on new entries. Note our own monitor lives here — baseline it first |
| SIP / FileVault / Gatekeeper disabled | `csrutil status`, `fdesetup status` as periodic posture checks |
| Unexpected removable device mount | New device in mount inventory, `removable` flag set |
| Monitoring daemon killed to hide activity | **Heartbeat gap.** Server Spec §4.1 `node_up` + §9 node-offline alert already cover this — say so explicitly, it is an anti-tamper control, not just a liveness check |
| Unauthenticated inference API exposed (Exo/OpenClaw) | Out of filesystem scope, but worth a port check — these ship with no auth and compute hijack is the motive |

### 2.4 Agentic AI risk — specific to this deployment

The challenge's own framing (OpenClaw, Exo, always-on local agents) introduces a
threat class worth calling out, because it is the most likely *real* incident on a
cluster like this and no generic filesystem tool addresses it:

An autonomous agent running 24/7 with shell access and filesystem write
permissions is a fundamentally different risk from a chatbot. Under prompt
injection or simple misalignment it can delete directories, exfiltrate secrets, or
overwrite shared data — while holding entirely legitimate credentials.

The useful insight for detection: **a runaway agent is statistically *easier* to
catch than a careful human insider.** It operates at machine speed with unnaturally
regular inter-operation timing and no diurnal rhythm. Rate and periodicity features
catch it even when identity-based controls cannot, because the agent is not
impersonating anyone — it *is* the authorized user.

---

## 3. Detection architecture

Three layers, in the order they should be built. Each is useful alone, so a
half-finished stack still demos.

**Layer 1 — deterministic rules.** Thresholds from Server Spec §9, plus config
audits from §2.1 above and snapshot-deletion detection. No training, no false-positive
tuning, immediately explainable. *This layer alone satisfies the challenge requirement.*

**Layer 2 — per-entity statistical baselines.** For each (node, export) pair, keep a
rolling EWMA and a robust dispersion estimate (median absolute deviation, not standard
deviation — one incident otherwise poisons the baseline it is measured against). Alert
on sustained deviation, requiring N consecutive windows to fire. This is literally
Server Spec §4.8's "large changes in per-user write volume relative to that user's
baseline," and it is where most of the real detection value lives for the effort spent.

**Layer 3 — unsupervised model.** Isolation Forest over the feature vector below.
Catches multivariate weirdness that no single threshold does — an operation *mix* that
is unusual even though every individual counter sits inside its normal range.

---

## 4. The learned model, realistically

**The honest constraint: there is no labeled malicious data, and there will not be
by judging.** That rules out supervised training of any network. Anyone promising a
trained classifier by tomorrow is promising a model fit on nothing.

What is genuinely achievable, in order:

### 4.1 Feature vector

Per (node, export, window), computed as deltas between consecutive samples and
normalized to per-second rates:

```
read_ops, write_ops, remove_ops, rename_ops, create_ops,
setattr_ops, lookup_ops, readdir_ops, getattr_ops, commit_ops
rename_ratio      = rename / (read + write + 1)
remove_ratio      = remove / (read + write + 1)
write_read_ratio  = write / (read + 1)
lookup_miss_ratio = lkup_misses / (lkup_hits + lkup_misses + 1)
used_kb_delta, used_kb_slope_5m
rpc_errors, rpc_timeouts, rpc_retries
snapshot_count_delta
hour_of_day, day_of_week          # cyclical encoding
inter_op_interval_cv              # low variance ⇒ machine-driven, see §2.4
```

Every one of these is derivable from what `nfs-lab/daemon/nfs-monitor.sh` already
emits. No new collection work is needed to start.

### 4.2 Model choice

**Start with Isolation Forest** (scikit-learn, `contamination='auto'`). Trains on
unlabeled normal data in seconds, needs no GPU, handles mixed-scale features without
much preprocessing, and — importantly for judging — you can explain *why* a sample
scored anomalous by looking at split features.

**If there is time, add a small autoencoder** over the same vector (e.g. 18→8→3→8→18,
reconstruction error as the anomaly score). This is the "neural network" answer, it
trains on normal-only data, and it captures feature interactions Isolation Forest
misses. It is a genuine improvement, not a checkbox — but build it *second*, because
it needs more baseline data and more tuning to beat the simpler model.

**Do not** attempt an LSTM/sequence model. The data volume over one weekend cannot
support it and it will underperform the Isolation Forest while looking more impressive.

### 4.3 Getting labels: synthetic attack generation

This is the highest-leverage item in this document.

You cannot collect real attacks, but you can *generate* them against the test export
in minutes, which gives labeled malicious traces for validation — and, if you want it,
supervised training:

| Script | Simulates |
| --- | --- |
| Walk tree, read every file sequentially | Bulk exfiltration |
| Read file → write high-entropy content → rename with new extension | Ransomware |
| Recursive delete of a seeded tree | Mass deletion |
| `dd` large files until quota is hit | Storage abuse |
| Recursive `ls -R` / `find` with no reads | Recon traversal |
| Trickle-read 1 file/30s for hours | Low-and-slow exfiltration |
| `tmutil deletelocalsnapshots` | Snapshot destruction |

Run each against a seeded copy of the export while the daemon samples. Label the
resulting windows by which script was running. Now there is a test set, precision and
recall become measurable, and the demo can show detection happening live rather than
asserting it works.

**Generate a long clean baseline too** — normal cluster activity (model loading,
inference reads, checkpoint writes) is the negative class, and without it the
false-positive rate is unknown.

---

## 5. Evasion and honest limitations

State these in the writeup. Judges reward knowing where the tool ends.

- **Entropy evasion.** Encoding ciphertext as base64 keeps file entropy low. This is
  why the design leans on *operation-mix* features rather than entropy — an
  advantage worth stating explicitly.
- **Rate evasion.** An attacker staying under thresholds defeats Layer 1 and much of
  Layer 2. Long-window features are the partial answer; a sufficiently patient
  attacker wins. Say so.
- **No attribution under `-mapall`.** See §1.1. Detection says *what happened on
  which export*, not *who did it*.
- **Aggregate counters.** `nfsstat -s` does not break down per client, so one noisy
  legitimate client can mask a quiet malicious one on the same export. Per-tenant
  exports (§1.1) are the structural fix.
- **Daemon tampering.** An attacker with root on a node can stop the collector. The
  heartbeat catches the gap but not what happened during it. Shipping samples
  off-host promptly bounds the loss.
- **Baseline poisoning.** An attacker who ramps up slowly can drag the baseline with
  them. MAD-based dispersion and a long-window comparison limit this; they do not
  eliminate it.

---

## 6. Open questions

- Per-tenant exports for structural attribution (§1.1) — worth the extra setup for
  the demo, or accept aggregate-only?
- Is `fs_usage` worth wiring in for process attribution, given it needs root and
  SIP hides protected processes?
- Does `nfsstat -n user|net` yield a real per-client breakdown under load? It returned
  the aggregate view on an idle host — needs retesting against a live export before
  anything is designed around it.
- Alert delivery: the brainstorming tab wants SMS to admins. Webhook first per
  Server Spec §12, Twilio second?

---

## Sources

- [NFS Security: What It Is, How It's Exploited & How to Defend It — Huntress](https://www.huntress.com/cybersecurity-101/topic/nfs)
- [NFS Security: Identifying and Exploiting Misconfigurations — HvS-Consulting](https://www.hvs-consulting.de/en/blog/nfs-security-identifying-and-exploiting-misconfigurations)
- [NfSpy — ID-spoofing NFS client](https://github.com/bonsaiviking/NfSpy)
- [NFS no_root_squash misconfiguration privilege escalation — HackTricks](https://hacktricks.wiki/en/linux-hardening/interesting-files-permissions/nfs-no_root_squash-misconfiguration-pe.html)
- [Writing a File Monitor with Apple's Endpoint Security Framework — Objective-See](https://objective-see.org/blog/blog_0x48.html)
- [Endpoint Security Overview — redcanaryco/mac-monitor wiki](https://github.com/redcanaryco/mac-monitor/wiki/5.-Endpoint-Security-Overview)
- [Ransomware detection based on server-side file operation logs using machine learning — Springer](https://link.springer.com/article/10.1186/s13635-026-00229-7)
- [Not on my watch: ransomware detection through classification of high-entropy file segments — Oxford Journal of Cybersecurity](https://academic.oup.com/cybersecurity/article/11/1/tyaf009/8109429)
- [MUSTARD: Adaptive Behavioral Analysis for Ransomware Detection — NEC Labs](https://neclab.eu/fileadmin/user_upload/Adaptive_Behavioral_Analysis_for_Ransomware_Detection.pdf)
- [Profiling the Invisible Insider: A UEBA-Based ML Framework for Low-and-Slow Data Exfiltration Detection — MDPI](https://www.mdpi.com/2076-3417/16/16/7952)
- [Deep Learning for Unsupervised Insider Threat Detection in Structured Cybersecurity Data Streams — arXiv](https://arxiv.org/pdf/1710.00811)
- [The Mac Mini as AI Server: OpenClaw, Open-Source Agents, and Always-On Agentic Infrastructure](https://medium.com/@brian-curry-research/the-mac-mini-as-ai-server-a-technical-guide-to-openclaw-open-source-agents-and-always-on-agentic-128521c3ea07)
- [The Guide to Managing Mac Clusters for AI Workloads — IRU](https://www.iru.com/blog/managing-mac-clusters)
