// The worker of the page /worker (W59): counts the primes up to n and fetches a file, then answers.
self.onmessage = function (e) {
  var n = e.data.n, count = 0;
  for (var i = 2; i <= n; i++) {
    var prime = true;
    for (var j = 2; j * j <= i; j++) if (i % j === 0) { prime = false; break; }
    if (prime) count++;
  }
  fetch('/assets/worker-data.json').then(function (r) { return r.json(); }).then(function (j) {
    postMessage({ n: n, count: count, name: self.name, fetched: j.ok, doc: typeof document, date: e.data.since instanceof Date });
  }, function (err) {
    postMessage({ n: n, count: count, error: String(err) });
  });
};
