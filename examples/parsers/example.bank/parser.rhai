// Synthetic example: the smallest useful mail parser.
//
// Copy this directory to test the corpus without real bank mail. The fixture is
// a single `.eml` whose body contains these four label blocks.

fn meta() {
    #{
        bank: "example",
        currency: "USD",
        tz: "UTC",
        offset: "+00:00",
        number: "en",
        kind: "purchase",
        labels: ["Amount", "Merchant", "Transaction Date", "Status"],
        api: 1,
        version: 1,
    }
}

fn parse_mail(ctx) {
    let status = ctx.text_in("Status");
    #{
        amount: ctx.money_regex_in("Amount", `\d[0-9.,]*`, "en"),
        occurred_at: ctx.datetime_in("Transaction Date", ["%d %b %Y %H:%M:%S"], "en"),
        counterparty: ctx.text_in("Merchant"),
        status: if status == "Successful" { "settled" }
                else if status == "Failed" { "failed" }
                else { "unknown" },
    }
}
