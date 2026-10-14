// CoinPrices: spot prices from Coinbase.
//
//   getPrice(pair)  → number, fetched now (pair like "BTC-USD")
//   watch(pair)     → start refreshing pair every poll; emits priceChanged(pair, price) at once
//   unwatch(pair)   → stop
//   priceChanged(pair, price)   a watched price changed
//   priceError(pair, message)   a watched price could not be fetched
//
// State lives in `this`, which is reset whenever the widget's page reloads.

fn init() {
    this.watched = #{};   // pair → last price (or () before the first fetch)
}

fn handle(method, args) {
    switch method {
        "getPrice" => fetch_price(pair_arg(args)),
        "watch" => {
            let pair = pair_arg(args);
            if !this.watched.contains(pair) {
                this.watched[pair] = ();
                this.refresh(pair);
            }
        }
        "unwatch" => { this.watched.remove(pair_arg(args)); }
    }
}

fn poll() {
    for pair in this.watched.keys() {
        this.refresh(pair);
    }
}

// Fetch one watched pair and emit if its price changed. Called with `this` bound.
fn refresh(pair) {
    try {
        let price = fetch_price(pair);
        if this.watched[pair] != price {
            this.watched[pair] = price;
            emit("priceChanged", [pair, price]);
        }
    } catch (e) {
        emit("priceError", [pair, `${e}`]);
    }
}

fn pair_arg(args) {
    if args.len() == 0 || type_of(args[0]) != "string" {
        throw "expected a pair such as \"BTC-USD\"";
    }
    let pair = args[0].to_upper();
    pair.trim();
    for c in pair.chars() {
        if !((c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '-') {
            throw `bad pair "${pair}"`;
        }
    }
    pair
}

fn fetch_price(pair) {
    let body = parse_json(http_get(`https://api.coinbase.com/v2/prices/${pair}/spot`));
    parse_float(body.data.amount)
}
