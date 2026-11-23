# Parser authoring pitfalls

## Blocks and labels

- **Missing label in `meta.labels`.** The previous region swallows the next
  field, so an amount matches a date or a phone number. Include *all* constant
  label blocks, even ones you do not extract.
- **Extra label in `meta.labels`.** A value that happened to be constant across
  the samples is treated as a label and disappears from its region.
- **`text_in` on a multi-value region.** A region can hold several blocks (rows,
  address lines). Use `block_in(label, n)` when the layout is positional.
- **Fused runs.** `cell` concatenates runs; `words` inserts spaces. Use `words`
  for descriptions, `cell` for geometry.

## Values

- **End-anchored amount regex.** `[\d.,]+$` captures the last number in the
  region, which is often a fee or a footer reference. Match the shape instead.
- **Number style.** `1.234,56` is `id`; `1,234.56` is `en`. The wrong choice
  scales the amount by 100.
- **Locale.** Indonesian months (`Okt`, `Agu`, `Mei`, `Des`, `Oktober`,
  `Agustus`) need `locale = "id"`. English month names always work.
- **Trailing zone tokens.** `13 Sep 2026 12:44:09 WIB` is fine with
  `datetime_in`: the helper tries every token window. A captured value that still
  fails means the format is wrong, not the helper.
- **Using the email `Date:` header as the transaction date.** The header is
  delivery time; the body carries the transaction timestamp. The header only
  supplies the offset.

## Semantics

- **Subject-based status.** There is no subject routing and no subject field in
  the host API. If the outcome is only in the subject, the label must
  distinguish it or the template is not extractable.
- **`account` guessed.** The ledger account is configured per label. The parser
  has no account field; never write a UUID into a script.
- **Wrong `kind`.** `purchase`, `transfer_out`, `fee`, and `cash_withdrawal` are
  outflows; `transfer_in` and `refund` are inflows. The sign comes from `kind`,
  not from a printed minus sign.
- **`version` not bumped.** Reusing a version after a behaviour change makes
  historical rows indistinguishable. Bump it.
- **One parser for several layouts.** If the samples under a label are different
  shapes, the label is mislabeled. Fix gmailctl.

## Script pitfalls

- **Unknown host function.** Rhai is dynamically typed, so a typo fails at the
  call site, not at load time. `abmx parser check` executes every fixture; run
  it before trusting a parser.
- **Forgetting `api`.** The default is 1. Bump it only when a script is written
  against a newer host API.
- **Returning a `Transaction`.** You cannot: the host accepts plain maps (or an
  array of maps). Identity and `strict` are host-owned.
- **Assuming one sample is enough.** A value that is constant in one message
  looks exactly like a label. Three samples are the minimum; more are better.

## Statements

- A row without a printed balance is normal: the host walks it. Only the
  printed balances, the totals, and the closing balance have to agree.
- Reading the date from the description column instead of the date column makes
  rows disappear or duplicate.
- A marker column is not optional if the bank uses it; without it every row is
  a credit and the debit total fails.
- Do not weaken the arithmetic to make a fixture pass.
