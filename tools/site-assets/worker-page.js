// The page /worker of tools/w24-site.py (W59): a Web Worker counts primes and fetches a file, the page
// shows the answer. Prints what it gets on the console (the serial log of the QEMU run).
(function () {
  var out = document.getElementById('out');
  var t0 = performance.now();
  var w = new Worker('/assets/worker-job.js', { name: 'primos' });
  w.onmessage = function (e) {
    console.log('worker: page got ' + JSON.stringify(e.data) + ' in ' + Math.round(performance.now() - t0) + ' ms');
    out.textContent = 'Primos ate ' + e.data.n + ': ' + e.data.count + ' (contados no worker "' + e.data.name + '")';
    document.title = 'Worker: ' + e.data.count;
    w.terminate();
  };
  w.onerror = function (e) { console.log('worker: error ' + e.message); };
  w.postMessage({ n: 20000, since: new Date(0) });
  console.log('worker: started ' + typeof Worker);
})();
