# The Torentino BitTorrent Manual

An all-in-one reference for how BitTorrent works — from the `.torrent` file down to the bytes on the wire. Written to guide a from-scratch implementation, one layer at a time.

---

## 1. Core concepts

**Pieces & blocks.** Torrent content is split into *pieces* (typically 256 KiB–1 MiB; the length is declared in the metainfo). Pieces are the unit of *integrity* — each has a SHA-1 hash stored in the `.torrent`. During transfer, a piece is further split into **16 KiB blocks**, so many peers can upload parts of the same piece in parallel.

**The swarm.** All peers downloading and uploading the same torrent. The point of BitTorrent: when many people download the same file, they upload to each other, so the source handles massive load with almost no extra cost.

**The infohash.** SHA-1 of the exact raw bencoded `info` dictionary. It is the torrent's identity everywhere: magnet links, DHT key, tracker announce, and the peer-wire handshake.

**Bencode** — the serialization format (4 types):
- integer → `i<number>e`  e.g. `i262144e`
- byte string → `<length>:<bytes>`  e.g. `4:spam`
- list → `l<items>e`  e.g. `l4:spam3:egge`
- dict → `d<keys, sorted>e`  e.g. `d4:name5:valuee`

Dict keys must be sorted (as raw strings, not alphanumerics). Integers reject leading zeros (`i03e` invalid) and `i-0e` invalid. All text in a `.torrent` is UTF-8.

---

## 2. The `.torrent` metainfo file

A `.torrent` file is one **bencoded dictionary** — nothing else. It contains *no content*, only metadata.

### Top-level keys

| key | type | meaning |
|---|---|---|
| `announce` | string | primary tracker URL |
| `announce-list` | list of lists | tracker *tiers* — try first tier, only fall to next if none work |
| `comment` | string | optional human note |
| `created by` | string | client that made it |
| `creation date` | int | unix timestamp |
| `info` | dict | **the critical part** — described below |

### The `info` dict

| key | type | meaning |
|---|---|---|
| `name` | string | file name (single) or folder name (multi) |
| `piece length` | int | bytes per piece (typically 16 KiB–16 MiB; 256 KiB–1 MiB common) |
| `pieces` | string | concatenated 20-byte SHA-1 hashes — one per piece, so `length == 20 × piece_count` |
| `length` | int | *single-file:* total file size |
| `files` | list | *multi-file:* `{length, path: [dir, ..., filename]}` per file |
| `private` | int | `1` = private torrent: DHT/PEX forbidden, tracker-only |
| `name.utf-8`, `path.utf-8` | string | UTF-8 variants of the above |

### Single vs multi-file

- **Single file**: `info` has `length` directly.
- **Multi file**: `info` has `files`; each entry has `length` and `path` (list of path components). All files are placed under the `name` folder.

### Concrete example

A 9 MB file `debian.iso`, 256 KiB pieces → `ceil(9209314 / 262144) = 36` pieces → `pieces` is 720 bytes of concatenated hashes:

```
d
  8:announce
    32:http://tracker.example.com/announce
  13:creation date
    i1454923576e
  4:info
    d
      6:length
        i9209314e
      4:name
        10:debian.iso
      12:piece length
        i262144e
      6:pieces
        720:<36 × 20-byte SHA-1 hashes, raw>
      e
e
```

### The infohash (the whole point)

> **Infohash = SHA-1 of the *exact raw bencoded bytes* of the `info` dict.**

Two `.torrent` files describe the same torrent **iff** their `info` dicts hash equal — even if `announce`, `comment`, etc. differ. So the parser must preserve the exact raw `info` bytes (don't decode→re-encode) or the hash won't match. If you do decode/re-encode, it's only safe when the decoder *fully validates* input (key ordering, no leading zeros); otherwise hash the extracted substring directly.

**Torrent v2 (BEP 52):** same shape, but `info` gains `meta version: 2`, replaces `pieces` with a SHA-256 Merkle tree (`file tree`), and the infohash becomes SHA-256 (in magnet links: `xt=urn:btmh:<multihash>`). Support v1 first; almost everything in the wild is v1 or a v1+v2 hybrid.

---

## 3. Magnet links (BEP 9)

A magnet link contains just enough to *join the swarm* — no `.torrent` file needed:

```
magnet:?xt=urn:btih:<40-hex infohash>&dn=<display name>&tr=<tracker>&x.pe=<peer:port>
```

- `xt` is the only mandatory parameter: the infohash (40 hex chars; also accept 32-char base32 for compatibility).
- `dn` display name, `tr` tracker URL (repeat for multiple), `x.pe` explicit peer address (repeatable) — all optional.
- If no tracker is given, the client SHOULD use the DHT to find peers.

The client finds peers (DHT/trackers), then fetches the metadata from a peer via **Metadata Exchange** (section 9). Once it has the `info` dict it's exactly as if it had the `.torrent` file.

---

## 4. The Peer Wire Protocol (BEP 3)

The byte-level language two peers speak over TCP. Everything — downloading, seeding, metadata exchange — rides on this framing.

### Transport and symmetry

- Runs over **TCP** (the uTP variant, BEP 29, is a drop-in replacement transport).
- Connections are **symmetrical**: both sides send the same messages; data flows both ways. There is no server/client role in the protocol.

### The handshake (68 bytes)

```
1  byte   pstrlen        = 19
19 bytes  pstr           = "BitTorrent protocol"
8  bytes  reserved       = capability flags
20 bytes  info_hash      = the torrent's infohash (raw, not hex)
20 bytes  peer_id        = your client identifier
```

- `0x13` (19) + the ASCII string `BitTorrent protocol` — a length-prefix scheme so other protocols are trivially distinguishable.
- **reserved bytes**: individual *bits* advertise optional protocol support. The ones you'll use:
  - Extension Protocol (BEP 10): `reserved[5] & 0x10`
  - DHT (BEP 5): the *last bit* → `reserved[7] & 0x01` (if set, send a `port` message with your UDP DHT port)
  - Fast Extension (BEP 6): `reserved[7] & 0x04`
- **info_hash**: the identity check. If it doesn't match the torrent you expect, **sever the connection**. Both sides must send the same value.
- **peer_id**: 20 bytes identifying *that peer's client* (e.g. `-TR3000-...`). If it doesn't match what the tracker told you, sever.

Example handshake bytes (infohash of all `aa` bytes, extension bit set):

```
13 42 69 74 54 6f 72 72 65 6e 74 20 70 72 6f 74 6f 63 6f 6c   "BitTorrent protocol"
00 00 00 00 00 10 00 00                                        reserved[5]&0x10 = ext on
aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa    info_hash
<20-byte peer_id>
```

### Message framing

After the handshake comes an **unending stream of length-prefixed messages**:

```
4 bytes   length      (big-endian) — bytes of the message *after* this prefix
1 byte    id          message type
n bytes   payload     message-specific
```

- `length == 0` → **keepalive**. Send one every ~2 minutes so NATs/proxies don't kill idle connections; ignore incoming ones.
- All integers inside payloads are **4 bytes big-endian**.
- TCP gives partial reads: you must buffer bytes until you have a full `4 + length` frame (length-prefixed framing parser).

### The message table

| id | name | payload |
|---|---|---|
| 0 | `choke` | none |
| 1 | `unchoke` | none |
| 2 | `interested` | none |
| 3 | `not interested` | none |
| 4 | `have` | 4-byte piece index |
| 5 | `bitfield` | bitfield bytes (only ever the *first* message) |
| 6 | `request` | `index`, `begin`, `length` (all 4 bytes) |
| 7 | `piece` | `index`, `begin`, `block` bytes |
| 8 | `cancel` | `index`, `begin`, `length` (like request) |
| 9 | `port` | 2-byte UDP port (DHT, BEP 5) |
| 13 | `suggest piece` | 4-byte index (BEP 6) |
| 14 | `have all` | none (BEP 6) |
| 15 | `have none` | none (BEP 6) |
| 16 | `reject request` | `index`, `begin`, `length` (BEP 6) |
| 17 | `allowed fast` | 4-byte index (BEP 6) |
| 20 | `extended` | 1-byte extension id + payload (BEP 10) |

Details:
- **bitfield**: one bit per piece, **high bit first** within each byte; spare trailing bits zeroed. Send it as your first message; skip it entirely if you have nothing.
- **request**: `index` = which piece, `begin` = byte offset inside the piece, `length` = bytes. The de-facto block size is **2^14 = 16 KiB**; peers close connections that request more.
- **piece**: correlates to a request *implicitly* (same index/begin). An unexpected `piece` can legitimately arrive if choke/unchoke race — don't assume it's an error (without fast extension).
- **cancel**: mainly used at the very end (endgame), see section 7.

Example request frame (piece 0, offset 0, 16 KiB):

```
00 00 00 0d     length = 13 (1 id + 12 payload)
06              request
00 00 00 00     index   = 0
00 00 00 00     begin   = 0
00 00 40 00     length  = 16384
```

Keepalive is just `00 00 00 00`.

### The choke/interest state machine

Each connection carries two independent bits of state on **each end**:

- **choked / unchoked** — "I will/won't send you data"
- **interested / not interested** — "I want data from you"

**Data flows only when the receiver is `interested` AND the sender is `unchoked`.** Both sides start **choked + not interested**.

You must keep your `interested` flag accurate at all times: as soon as a peer holds no piece you want, send `not interested` even if choked — it tells the peer whether unchoking you is worth anything.

### Pipelining

Don't send one request at a time — that's a round-trip per block over TCP. Keep **several requests in flight** (libtorrent advertises ~250 via `reqq`). On the sender side, requests that can't flush to the socket get queued in memory so they can be **discarded when you choke** the peer.

### The full download flow on one connection

```
connect TCP ──► exchange handshakes
            ──► send bitfield (what I already have)
            ──► we both send interested/not-interested
            ──► peer unchokes me
            ──► I pipeline request messages
            ──► peer sends piece messages
            ──► I assemble blocks → verify piece SHA-1 → broadcast have
            ──► (later) I unchoke them → they request from me → seeding
```

---

## 5. Trackers — peer discovery

A tracker is a well-known server that answers **"who else is in this swarm?"** Two kinds: HTTP(S) and UDP.

### HTTP tracker announce (BEP 3)

A `GET` request with these query params:

| param | meaning |
|---|---|
| `info_hash` | the 20-byte infohash, **URL-escaped raw bytes** |
| `peer_id` | your 20-byte id, URL-escaped |
| `port` | the TCP port you're listening on |
| `uploaded` | total bytes uploaded so far |
| `downloaded` | total bytes downloaded so far |
| `left` | bytes still to download (resume-aware, can't derive from size) |
| `event` | optional: `started` \| `completed` \| `stopped` |
| `compact` | `1` to request compact peer encoding |
| `key` | optional random key identifying you across requests |
| `numwant` | optional: number of peers desired |

**Announce lifecycle:** send `started` when you begin, `completed` when the download finishes (only if it wasn't already complete at start), `stopped` when you leave. Otherwise re-announce every `interval` seconds.

**Response** — a bencoded dict:
- `failure reason` — string; present = failed, no other keys required.
- `interval` — seconds to wait between regular re-announces.
- `min interval` — optional lower bound.
- `peers` — either a list of dicts (`peer id`, `ip`, `port`) or, with `compact=1`, a **compact peer string**: 6 bytes per IPv4 peer (4-byte IP + 2-byte port, network byte order); 18 bytes per IPv6 peer.
- Also commonly present: `tracker id`, `complete` (seeders), `incomplete` (leechers).

### UDP tracker protocol (BEP 15)

Binary, big-endian, over UDP. Requires a two-step handshake to defeat source-spoofing: the tracker issues a `connection_id` only a client that receives its own reply could have.

**1. Connect** — get a connection id:
```
connect request:   protocol_id (64-bit) = 0x41727101980 | action (0) | transaction_id
connect response:  action (0) | transaction_id | connection_id (64-bit)
```
The connection id is usable for ~1 minute (trackers accept ~2). It can be reused for many announces.

**2. Announce**:
```
IPv4 announce request:
  connection_id (64) | action (1) | transaction_id (32) | info_hash (20)
  | peer_id (20) | downloaded (64) | left (64) | uploaded (64)
  | event (32: 0 none, 1 completed, 2 started, 3 stopped)
  | IP (32, 0 = default) | key (32) | num_want (32, -1 = default) | port (16)

IPv4 announce response:
  action (1) | transaction_id | interval (32) | leechers (32) | seeders (32)
  | then 6-byte compact peers (IP + port), repeated
```
IPv6 announces use the same structure but the response stride is 18 bytes/peer.

**3. Scrape** (optional) — stats per infohash: `seeders, completed, leechers` (12 bytes each).

**4. Errors:** action `3` + transaction id + message string.

**Retransmission:** UDP drops packets, so you must retransmit after `15 × 2^n` seconds (n from 0 up to 8 = 3840s). Re-fetch the connection id when it expires. Do not re-announce before `interval` has passed (unless an event fires).

### Multi-tracker (BEP 12)

`announce-list` groups trackers into **tiers** — list of lists. Query one tier; only fall to the next tier if none in the current tier work. Within a tier you may query multiple, but beware announcing to too many.

---

## 6. Choking & tit-for-tat (BEP 3)

Choking is the fairness mechanism. Unchoking lets you download from a peer. The deployed algorithm:

- **Cap simultaneous uploads** for good TCP behavior (TCP congestion control degrades over many connections).
- **Recalculate once every 10 seconds** — avoids "fibrillation" (rapid choke/unchoke cycles).
- **Reciprocate**: unchoke the **four peers with the best download rates** that are currently interested. (If you're a seeder, use *upload* rate instead.)
- **Optimistic unchoke**: at any time, exactly **one** peer is unchoked regardless of rate, to try out connections and let new peers start. It **rotates every 30 seconds**; new connections are **3× more likely** to start as the current optimistic unchoke.
- Peers with a good upload rate but not interested may be unchoked so they become interested; if they do, the worst uploader gets choked.

Why it works: downloaders who upload get unchoked in return — tit-for-tat. Free-riders get snubbed and can only download during the brief optimistic-unchoke windows.

---

## 7. Piece selection

The strategy for *what to request next*:

- **Rarest first** — request the piece that the *fewest* peers have. Maximizes swarm health: rare pieces spread, and no piece goes missing.
- **Random order** — the baseline (BEP 3 describes downloading in random order); a decent job of avoiding strict subset/superset relationships between peers.
- **Random first piece** — at the very start, grab a random complete piece fast so you have something to trade (combined with allowed-fast, section 8).
- **Sequential** — for streaming/playback: prioritize pieces in file order.
- **Endgame mode** — when the download is nearly complete, the last few pieces can crawl on a slow peer. Once requests are pending for *all* missing pieces, send requests for everything to *everyone*, and send **cancels** for redundant pieces as each arrives.

Practical notes:
- Pipeline many requests (section 4).
- After a peer chokes you, its pending requests are cancelled.
- Track a per-peer "interested" state: only request pieces that peer actually has.

---

## 8. Fast Extension (BEP 6)

Enabled by **both** ends setting `reserved[7] |= 0x04`. Packages several extensions:

- **Have All / Have None** (ids 14/15): replace the bitfield when you have all/none of the pieces — saves a message. Exactly one of *Have All / Have None / Have Bitfield* must appear, immediately after the handshake.
- **Reject Request** (16): now every request gets **exactly one** response — a `reject` or a `piece`. Choke no longer implicitly rejects pending requests. If you receive a piece you never requested, **close the connection**.
- **Suggest Piece** (13): advisory "you might like this piece" — used for super-seeding and to avoid redundant downloads.
- **Allowed Fast** (17): "if you ask for this piece, I'll give it to you **even while choked**." New peers have few pieces to trade, so this jump-starts tit-for-tat. The allowed-fast set is computed per-peer by a canonical algorithm:

```
x = (ip & 0xffffff00) as 4 bytes   // only the top 3 octets of the peer's IP
x.append(infohash)
while len(set) < k:                  // k = 10
  x = SHA1(x)
  for each of the 5 groups of 4 bytes in x:
    index = (that 4-byte value) % piece_count
    add index to set if not already present
```

Note: an Allowed Fast message does **not** mean the sender has that piece.

---

## 9. Extension Protocol (BEP 10) & Metadata Exchange (BEP 9)

### The extension transport (BEP 10)

A thin multiplexing layer so extensions can be added without version clashes:

- Advertised by `reserved[5] & 0x10` in the handshake.
- Uses standard message id **20** (`extended`). Payload starts with a **1-byte extension message id**:
  - id **0** = the extension **handshake**
  - id > 0 = an extension message as negotiated by that handshake
- The extension handshake is a **bencoded dict** sent immediately after the standard handshake:

```
{
  "m": { "ut_metadata": 3, "ut_pex": 2, ... },  // extension name → per-peer local id
  "p": 6881,                                    // my TCP listen port (optional)
  "v": "Torentino 0.1",                         // client string (optional)
  "reqq": 250                                   // outstanding requests I support (optional)
}
```

Key rules:
- Extension ids are **local to each peer** — the same extension may have different ids on different connections. Store them per connection and map back when sending.
- Setting an id to **0** disables that extension.
- Names must be prefixed with the client's code (e.g. `LT_metadata` from libtorrent, `ut_pex`, `ut_metadata`) to avoid collisions.

### Metadata Exchange (BEP 9) — how magnet links work

Purpose: download the `info` dict from a peer so you can join the swarm with only an infohash. The metadata *is* the `info` dict; it's validated by the infohash (SHA-1 over the assembled bytes must equal `xt=urn:btih:`).

- Advertised by adding `ut_metadata` to the handshake `m` dict, plus `metadata_size` at top level of the handshake (bytes of the metadata).
- The metadata is transferred in **16 KiB blocks** (last block may be smaller), indexed from 0.
- Three message types (payloads are bencoded dicts):

| `msg_type` | name | extra keys |
|---|---|---|
| 0 | `request` | `piece` (which block) |
| 1 | `data` | `piece`, `total_size` — followed by the raw metadata bytes appended to the dict |
| 2 | `reject` | `piece` |

- A peer without the full metadata MUST `reject` every metadata request (it can't verify partial metadata).
- The `data` message is a bencoded dict **plus raw bytes appended after it** (both included in the message length prefix, the raw bytes are *not* part of the dict).
- Validate: once assembled, `SHA1(all metadata bytes) == infohash`. Only then trust it as the torrent's `info`.

### The magnet download flow, end to end

```
parse magnet → infohash X (and trackers from tr=)
      │
      ▼  find peers: DHT get_peers(X)  AND/OR  announce to trackers
      ▼  connect via peer-wire protocol
      ▼  extension handshake (BEP 10) → peer advertises ut_metadata
      ▼  Metadata Exchange (BEP 9): request blocks until the info dict is complete
      ▼  verify SHA-1 == infohash → you now have the metadata (= a .torrent file)
      ▼  announce_peer / announce to register yourself in the swarm
      ▼  download pieces normally
```

---

## 10. DHT — trackerless peer discovery (BEP 5)

DHT is the **tracker replacement**. A tracker is one server that answers "who has this infohash?" DHT spreads that question across thousands of random peers over UDP. It's a *distributed hash table*: key = 160-bit infohash, value = list of `IP:port` of peers that have the torrent. It's built on **Kademlia**.

**1. Node IDs and distance**
- Every node picks a random 160-bit ID.
- "Distance" is **XOR**: `a XOR b`, interpreted as a number — not geographic, a *bitwise* closeness. Cheap to compute, consistent, symmetric.

**2. Routing table = k-buckets**
- ~160 buckets. Bucket *i* holds nodes sharing the first `i` bits of the prefix (bucket 0 = nodes differing in the top bit).
- Each bucket holds up to **k = 8** nodes (`IP:port`, node-id, last-seen).
- New node: if the bucket isn't full, add; if full, ping the *oldest* and evict it if unresponsive.
- A bucket splits when *your own* node id falls in its range. Keep the table fresh by refreshing buckets not changed in 15 minutes (run a `find_node` for a random id in the bucket's range).

**3. The four RPCs** (KRPC: bencoded dicts over UDP; every message has `t` transaction id and `y` = `q`/`r`/`e`):

| message | args | response |
|---|---|---|
| `ping` | — | liveness |
| `find_node` | `target` ID | up to k closest nodes (compact: 26 bytes each = 20-byte id + 6-byte addr) |
| `get_peers` | `info_hash` | `values` = peer IP:ports **or** `nodes` (closer nodes) + a `token` |
| `announce_peer` | `info_hash`, `port`, `token` | "I have this torrent, I'm reachable here" |

Contact encoding: peer = 6 bytes (4-byte IP + 2-byte port; 18 bytes IPv6). Node = 26 bytes (20-byte id + peer address). All network byte order.

**4. The lookup algorithm** — the heart of it
To find peers for infohash `X`:
1. Pick the **α = 3** closest nodes you know.
2. Send them `get_peers(X)` in parallel.
3. Collect closer nodes into a shortlist, sorted by XOR-distance to `X`.
4. Query the next α closest *unqueried* nodes, repeat until you've queried the k closest.
5. Those nearest nodes store the mapping, because *every node keeps the entries for infohashes close to its own ID*. That invariant is what makes the navigation work: to find who has `X`, walk toward `X` in ID space.

**5. Bootstrapping**
A brand-new node knows nobody. It pings a **bootstrap node** (hardcoded well-known IPs like `router.bittorrent.com` / `router.utorrent.com` / `dht.transmissionbt.com`, port 6881) with a `find_node` for its own ID — filling the first buckets. Also: peers supporting DHT set the DHT reserved bit; when you see it, send a `port` message with your DHT UDP port, and ping them to grow your table.

**6. Security**
- `announce_peer` requires a **token** the queried node issued (hash of IP + a secret rotated ~5 min, tokens accepted ~10 min) so attackers can't fake announces from other IPs.
- **BEP 42**: node IDs must derive their high bits from a hash of the node's IP — hard to forge, mitigates address spoofing/Sybil attacks.

**7. Good vs bad nodes**
A node is "good" if it responded to a query in the last 15 minutes (or responded once and queried you within 15 min). A bucket full of good nodes discards newcomers; replace bad/questionable nodes by pinging the least-recently-seen questionable node.

---

## 11. Storage & piece-to-file mapping

**The virtual file.** For all mapping purposes, a torrent is treated as **one concatenated byte stream** built from the files in `files` order. Piece `i` covers byte range `[i × piece_length, (i+1) × piece_length)`, except the last piece, which may be truncated to the total size.

**Mapping a piece/block to disk:**
1. Compute the absolute byte offset: `offset = piece_index × piece_length + begin`.
2. Walk the file table (`length`, `path`) accumulating offsets until you find the file(s) containing `[offset, offset + blocklen)`.
3. A block can span two files (or cross a file boundary mid-piece) — write each slice to the right file.

**Single-file** is just one entry in that table.

**Writing policy** (choose one):
- *Write-then-verify:* write raw blocks to disk as they arrive, verify the SHA-1 only when the piece is complete, and on failure re-download that piece. Faster disk pattern.
- *Buffer-then-verify:* assemble the full piece in memory, verify, then write. Simpler to reason about; fine for small pieces.

**On successful verification:** mark the piece as have, write (if buffered), broadcast `have` to peers.

**Fastresume:** persist which pieces are verified (a bitfield) plus the infohash and paths. On restart, you can skip re-hashing every file and resume immediately — only the incomplete last piece needs re-verification. This is what makes restarting your client not cost an hour of SHA-1 work.

---

## 12. Swarm lifecycle: leech → seed → host

A client's life for one torrent:

1. **Start** — announce `started` (tracker) / `announce_peer` (DHT); join the swarm.
2. **Leech** — download pieces (section 7), upload whenever unchoked (section 6).
3. **Complete** — announce `completed` (only if it wasn't complete at start).
4. **Seed / host** — keep the connection alive and **serve `piece` requests**. You now *are* part of the infrastructure: other peers download from you. Seeder choking uses *upload* rates. This is what "hosting" means — staying in the swarm and uploading.
5. **Stop** — announce `stopped` and close connections.

**Fastresume** (section 11) lets a restart jump straight back to step 3/4 without re-hashing.

---

## 13. Client architecture

How the fundamentals map onto a real implementation:

```
┌──────────────────────────────────────────────────────────┐
│  Module / UI layer     Torentino façade, CLI, future web  │
├──────────────────────────────────────────────────────────┤
│  Session / Core        torrent registry, state machine,   │
│                        event bus, persistence             │
├──────────────────────────────────────────────────────────┤
│  Peer manager          connections, wire protocol,        │
│                        choke/unchoke policy               │
├──────────────────────────────────────────────────────────┤
│  Piece manager         block scheduling, piece selection, │
│                        endgame                            │
├──────────────────────────────────────────────────────────┤
│  Discovery             HTTP/UDP trackers, DHT, PEX, LSD   │
├──────────────────────────────────────────────────────────┤
│  Parsing               bencode, .torrent, magnet          │
├──────────────────────────────────────────────────────────┤
│  Storage               file mapping, disk I/O, piece      │
│                        verification                       │
└──────────────────────────────────────────────────────────┘
```

Key rules:
- **Parsing is pure** — everything else depends on it.
- **Core** keeps one `Torrent` object per infohash with a state machine (`queued → downloading → done → seeding → paused → error`) and an event emitter so the module/UI can subscribe.
- **Peer manager** per connection: handshake, framing parser, interest/choke state, message handlers.
- **Piece manager** tracks per-piece state (missing / in-flight / have) and decides requests; gets `have`s and bitfields from the peer manager.

---

## 14. Learning order & references

Recommended build order (each step ends working + tested):

1. Bencode encode/decode
2. `.torrent` parsing + infohash computation
3. Storage layer (piece→file mapping, SHA-1 verification)
4. HTTP tracker announce → peer list
5. Peer wire protocol (handshake, messages, keep-alive)
6. Piece manager (blocks → pieces, bitfield, rarest-first)
7. **End-to-end single-file download from real peers**
8. Seeding (serve pieces, accept connections)
9. Multi-file torrents + fastresume
10. Magnet links via DHT + Metadata Exchange (BEP 9/10)
11. UDP tracker (BEP 15), then DHT (BEP 5)
12. Module API + CLI + persistence

References:
- BEP index: https://www.bittorrent.org/beps/
- BEP 3 (protocol spec), BEP 5 (DHT), BEP 6 (fast ext), BEP 9 (metadata), BEP 10 (extensions), BEP 15 (UDP tracker), BEP 23 (compact peers), BEP 42 (node id), BEP 52 (torrent v2)
- Protocol wiki: https://wiki.theory.org/BitTorrentSpecification
- Readable reference implementations: WebTorrent's `bittorrent-protocol` & `bittorrent-dht` (MIT), rqbit's `crates/dht` and `crates/peer_binary_protocol` (Rust), `torrent-client` (from-scratch Go client)
- Wireshark's BitTorrent dissectors are invaluable for debugging against real clients.
