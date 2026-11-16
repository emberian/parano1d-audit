# Development-fund payouts and balances: one explorer view

Read-only public explorer queries at about 2026-11-16 03:32 UTC
(2026-11-15 19:32 PST). The raw JSON responses are in
[`public-fund-snapshot-2026-11-15/`](public-fund-snapshot-2026-11-15/).
The explorer reported tip height **313,800** in its economics response. This
is a single service's view, queried over several seconds, not an independently
verified chain-state proof or a claim about later balances.

The [pinned source](../../parano1d/noid_chain/src/consensus/development_allocation.rs)
hardcodes the O(1) Network Fund and Parano1d Lab payout addresses; its own
address test prints the corresponding `o1…` encodings at lines 250–259.
The [official economics page](https://docs.parano1d.org/protocol/economics)
documents the two 5% subsidy streams. The explorer's
[`/api/v1/economics`](https://noidexplorer.org/api/v1/economics) reported
`cumulative_per_fund_micronoid = 599040000000`, or **599,040 NOID issued to
each fund** through its tip. That is a protocol payout total, not a balance
available at the original address.

| Address or output | Explorer observation | Evidence |
| --- | ---: | --- |
| [O(1) Network Fund payout address](https://noidexplorer.org/api/v1/address/o1r3djxaz54k47kr54x76cwqkhl6xqucescv943m5m8uvc5w6x7euqct3h07) | 61,047.4398 NOID live balance | `fund.json`, `live_balance_micronoid` |
| [Parano1d Lab payout address](https://noidexplorer.org/api/v1/address/o1xcjdp3udp5sgwcvnmjlu9s53u4fx5m3hpquvg0uem2pr2mrr9dqq38jple) | 61,112.241309 NOID live balance | `lab.json`, same field |
| [Recipient of Fund transfer](https://noidexplorer.org/api/v1/address/o1rv9yev2ugpqq5r56rnrwhrrlpquqc2a0x3pxwul5tla3ws053ekqy39ngw) | 500,000 NOID live balance, one recorded receipt | `fund_500k_recipient.json`; Fund tx `6fa20222…6b73f` at H225,854 |
| [Recipient of Lab transfer](https://noidexplorer.org/api/v1/address/o1m0p7jue04wjwp0qq2psnrctuyr4etv5gjlyz9v8cv2m2tyd62dgqllv4q3) | 500,000 NOID live balance, one recorded receipt | `lab_500k_recipient.json`; Lab tx `6428879d…725f31d` at H225,859 |

The two 500,000-NOID outputs together equal about **8.22%** of the
explorer's reported 12,158,714.2425 NOID net supply at this tip. They
remain unspent *in this explorer view*. Their separate recipient addresses
do not prove one controller, custody by either fund, an exchange deposit,
or any special network power. They could represent cold storage or transfers
to independent parties; the public transactions alone do not distinguish
those cases. The original payout addresses' smaller live
balances must not be mistaken for the total fund issuance.

The explorer's address history has older coverage gaps and gross
`total_received`/`total_sent` figures that count change and previously
unrecorded inputs. This report uses only the cited live-balance fields, the
two explicit 500,000-NOID transfer records, and the consensus payout total.
No personal identity was inferred from an address. This observation describes
the distribution of issued funds in one explorer view. It does not establish
their custody, trading activity, or an effect on other holders' deposits.
