# Parser authoring pitfalls

- **Missing label in `labels.list`.** The region for the previous label swallows
  the next field, so the amount regex matches a date or a phone number. Include
  *all* constant label blocks, even ones you do not extract.
- **Extra label in `labels.list`.** A value that happened to be constant across
  the samples is treated as a label and disappears from its region.
- **End-anchored amount regex.** `[\d.,]+$` captures the last number in the
  region, which is often a fee, a phone number, or a footer reference. Match the
  shape instead.
- **Number style.** `1.234,56` is Indonesian (`number = "id"`); `1,234.56` is
  English (`number = "en"`). The wrong choice scales the amount by 100.
- **Locale.** Indonesian months (`Okt`, `Agu`, `Mei`, `Des`) need
  `locale = "id"` in `[extract.occurred_at]`.
- **Using the email `Date:` header as the transaction date.** The header is
  delivery time. The body carries the transaction timestamp; the header only
  supplies the timezone offset.
- **Subject regex not anchored.** `subject = 'Payment'` also matches
  `Payment failed`. Anchor with `^...$` and use `[status]` for the outcome.
- **Status subject capture missing.** `source = "subject"` requires a named
  capture called `status` in the subject regex.
- **One parser for several layouts.** If block counts differ across samples,
  they are probably different templates. Split them and use `requires` /
  `rejects`, or ask the user which label grouping is correct.
- **`account` guessed.** `[defaults].account` is a key the user must map in
  txsync's `[accounts]`. Do not invent a mapping, and never write a ledger UUID
  into a parser file.
- **Wrong `kind`.** `purchase` is an outflow, `transfer_in` an inflow,
  `transfer_out` an outflow, `fee` an outflow, `refund` an inflow. The sign of
  the transaction is derived from `kind`, not from the printed minus sign.
- **`version` not bumped.** Reusing the same `version` after changing behaviour
  makes historical rows indistinguishable. Bump it.
- **A spec that matches too broadly.** A wildcard sender plus a loose subject can
  hijack another bank's mail. Prefer exact senders and add `rejects` when a
  sibling template shares the sender.
