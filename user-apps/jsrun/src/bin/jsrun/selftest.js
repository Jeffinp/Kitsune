// jsrun --selftest: checks the engine and its C library under Kitsune, then times a small benchmark.
// Every check prints "ok" or "FAIL"; the last line says how many passed. Runs as global code with
// top-level await (jsrun evaluates scripts that way).
let passed = 0, failed = 0;
function check(name, cond, got) {
  if (cond) { passed++; console.log("ok   " + name); }
  else { failed++; console.log("FAIL " + name + (got === undefined ? "" : " (got " + String(got) + ")")); }
}

// Arithmetic and the number <-> string conversions (QuickJS's dtoa, musl's libm on SSE2).
check("arith: 0.1 + 0.2", 0.1 + 0.2 === 0.30000000000000004 && String(0.1 + 0.2) === "0.30000000000000004");
check("arith: 1/3", String(1 / 3) === "0.3333333333333333", 1 / 3);
check("arith: integers", 7 * 6 === 42 && (2 ** 53) + 1 === 2 ** 53 && (-7 % 3) === -1 && (7 >>> 1) === 3);
check("arith: toFixed/toPrecision", (1.005).toFixed(2) === "1.00" && (123.456).toPrecision(4) === "123.5");
check("arith: radix", (255).toString(16) === "ff" && (0.5).toString(2) === "0.1" && parseInt("z", 36) === 35);
check("arith: parseFloat", parseFloat("3.14e2") === 314 && Number("0x1f") === 31 && isNaN(Number("x")));
check("math: sqrt", Math.sqrt(2) === 1.4142135623730951, Math.sqrt(2));
const near = (x, y) => Math.abs(x - y) <= 2e-16 * Math.max(1, Math.abs(y));
check("math: trig", near(Math.sin(Math.PI / 6), 0.5) && Math.cos(0) === 1 && near(Math.atan2(1, 1), Math.PI / 4) && near(Math.tan(Math.PI / 4), 1));
check("math: exp/log", near(Math.exp(1), Math.E) && near(Math.log(Math.E), 1) && Math.log2(1024) === 10 && near(Math.log10(1e5), 5));
check("math: pow/cbrt/hypot", near(Math.pow(2, 0.5), Math.SQRT2) && near(Math.cbrt(27), 3) && Math.hypot(3, 4) === 5 && 2 ** 10 === 1024);
check("math: rounding", Math.round(2.5) === 3 && Object.is(Math.round(-0.4), -0) && Math.floor(-1.5) === -2 &&
  Math.ceil(1.2) === 2 && Math.trunc(-4.7) === -4 && Math.sign(-3) === -1);
check("math: special", Math.max() === -Infinity && isNaN(Math.min(1, NaN)) && 1 / 0 === Infinity && Object.is(-0 * 1, -0));

// Strings and Unicode.
check("string: case", "ação".toUpperCase() === "AÇÃO" && "ÉCOLE".toLowerCase() === "école");
check("string: astral", [..."😀ab"].length === 3 && "😀".length === 2 && "😀".codePointAt(0) === 0x1F600);
check("string: normalize", "é".normalize("NFD").length === 2 && "é".normalize("NFC") === "é");
check("string: methods", "a-b-c".split("-").join("+") === "a+b+c" && "5".padStart(3, "0") === "005" &&
  "  x ".trim() === "x" && "abc".at(-1) === "c" && "x".repeat(3) === "xxx" && "abcabc".replaceAll("b", "") === "acac");
const who = "Kitsune";
check("string: template", `olá, ${who}! ${1 + 1}` === "olá, Kitsune! 2" && String.raw`a\nb` === "a\\nb");
check("string: utf-8 in source", "日本語".length === 3 && "日本語".charCodeAt(0) === 0x65E5);

// JSON.
const obj = { a: [1, { b: "x" }], c: null, d: true, e: 1.5, s: "aspas \" e ç" };
const text = JSON.stringify(obj);
check("json: stringify", text === '{"a":[1,{"b":"x"}],"c":null,"d":true,"e":1.5,"s":"aspas \\" e ç"}', text);
check("json: round trip", JSON.stringify(JSON.parse(text)) === text);
check("json: indent", JSON.stringify([1, 2], null, 1) === "[\n 1,\n 2\n]");
let jsonError = false;
try { JSON.parse("{bad"); } catch (e) { jsonError = e instanceof SyntaxError; }
check("json: syntax error", jsonError);

// Dates: Date.now() is UTC from the kernel (system call 9).
const now = Date.now();
check("date: now is plausible (after 2024)", now > Date.UTC(2024, 0, 1) && now < Date.UTC(2100, 0, 1), now);
check("date: epoch", new Date(0).toISOString() === "1970-01-01T00:00:00.000Z");
check("date: UTC fields", new Date(Date.UTC(2024, 1, 29, 12, 30)).toISOString() === "2024-02-29T12:30:00.000Z");
check("date: parse", Date.parse("2001-09-09T01:46:40Z") === 1e12);
const tz = new Date().getTimezoneOffset();
check("date: time zone offset is whole minutes", Number.isInteger(tz) && Math.abs(tz) <= 14 * 60, tz);
console.log("info Date.now() = " + now + " = " + new Date(now).toISOString() + " (offset " + tz + " min)");

// Regular expressions.
const m = /(\d+)-(\d+)/.exec("de 10-20 h");
check("regex: groups", m && m[1] === "10" && m[2] === "20" && m.index === 3);
const named = /(?<y>\d{4})-(?<mo>\d{2})/.exec("2026-10");
check("regex: named groups", named.groups.y === "2026" && named.groups.mo === "10");
check("regex: unicode property", /^\p{L}+$/u.test("ação") && !/^\p{L}+$/u.test("a1"));
check("regex: replace", "a1b22c333".replace(/\d+/g, (d) => "<" + d.length + ">") === "a<1>b<2>c<3>");
check("regex: lookbehind/flags", /(?<=R\$)\d+/.exec("R$42")[0] === "42" && /ABC/i.test("abc") && /^b/m.test("a\nb"));
check("regex: matchAll", [..."x1y2z3".matchAll(/\d/g)].map((r) => r[0]).join("") === "123");

// Classes.
class Animal {
  #name;
  static count = 0;
  constructor(name) { this.#name = name; Animal.count++; }
  get name() { return this.#name; }
  speak() { return this.#name + " faz barulho"; }
}
class Fox extends Animal {
  speak() { return super.speak() + ": ring-ding"; }
}
const fox = new Fox("raposa");
check("class: inheritance and private fields", fox.speak() === "raposa faz barulho: ring-ding" && fox.name === "raposa" &&
  Animal.count === 1 && fox instanceof Animal && !("#name" in fox));

// Generators and iterators.
function* fibGen() { let [a, b] = [0, 1]; for (;;) { yield a; [a, b] = [b, a + b]; } }
const firstFibs = [];
for (const f of fibGen()) { if (f > 50) break; firstFibs.push(f); }
check("generator: fibonacci", firstFibs.join(",") === "0,1,1,2,3,5,8,13,21,34");
function* delegate() { yield 1; yield* [2, 3]; return 4; }
check("generator: yield*", [...delegate()].join() === "1,2,3");

// Collections.
const map = new Map([["a", 1], ["b", 2]]);
map.set("c", 3).delete("a");
const set = new Set([3, 1, 3, 2, 1]);
check("collections: Map", map.size === 2 && [...map.keys()].join() === "b,c" && map.get("c") === 3);
check("collections: Set", set.size === 3 && [...set].join() === "3,1,2" && set.has(2));
const wm = new WeakMap(); const key = {}; wm.set(key, "v");
check("collections: WeakMap", wm.get(key) === "v" && !wm.has({}));

// BigInt.
check("bigint: power", (2n ** 100n).toString() === "1267650600228229401496703205376");
check("bigint: arithmetic", (10n ** 30n + 7n) % 1000000007n === 999657014n && -7n / 2n === -3n && BigInt(2 ** 53) + 1n === 9007199254740993n);
check("bigint: mixing throws", (() => { try { return 1n + 1; } catch (e) { return e instanceof TypeError; } })());

// The rest of the language that real pages lean on.
const { a: [first, { b }], ...others } = obj;
check("syntax: destructuring/rest", first === 1 && b === "x" && Object.keys(others).join() === "c,d,e,s");
check("syntax: optional chaining/nullish", obj?.z?.y === undefined && (obj.c ?? "def") === "def" && (0 || "x") === "x");
check("syntax: spread/arrow/default", ((x, y = 2, ...r) => x + y + r.length)(...[1, undefined, 9, 9]) === 5);
check("array: methods", [3, 1, 2].sort((x, y) => x - y).join() === "1,2,3" && [[1, [2]], 3].flat(2).join() === "1,2,3" &&
  [1, 2, 3].reduce((s, x) => s + x, 0) === 6 && [1, 2, 3].findLast((x) => x < 3) === 2 && Array.from({ length: 3 }, (_, i) => i * i).join() === "0,1,4");
check("array: typed", new Uint8Array([255, 256, 257]).join() === "255,0,1" && new Float64Array(2).length === 2 &&
  new DataView(new ArrayBuffer(4)).getInt32(0) === 0);
const proxy = new Proxy({}, { get: (t, k) => "proxied " + String(k) });
check("proxy/reflect/symbol", proxy.foo === "proxied foo" && Reflect.ownKeys({ x: 1, [Symbol.iterator]: 0 }).length === 2 &&
  typeof Symbol("s") === "symbol");
const order = [];
try { try { throw new Error("inner"); } finally { order.push("finally"); } } catch (e) { order.push(e.message); }
check("exceptions: finally order", order.join() === "finally,inner");
check("error: stack trace", (() => { try { null.x; } catch (e) { return e instanceof TypeError && /at /.test(e.stack); } })());

// Promises and async/await (the job queue runs between the awaits).
const log = [];
Promise.resolve().then(() => log.push("micro"));
log.push("sync");
const sleepish = (v) => new Promise((resolve) => resolve(v));
async function sum(n) { let t = 0; for (let i = 1; i <= n; i++) t += await sleepish(i); return t; }
check("async: await chain", (await sum(10)) === 55 && log.join() === "sync,micro", log.join());
const settled = await Promise.allSettled([Promise.reject(new Error("no")), Promise.resolve(2)]);
check("async: allSettled", settled[0].status === "rejected" && settled[1].value === 2);
check("async: rejection caught by await", await (async () => { try { await Promise.reject(7); } catch (e) { return e === 7; } })());
async function* agen() { yield 1; yield 2; }
let asum = 0;
for await (const v of agen()) asum += v;
check("async: for await", asum === 3);

// Deep recursion: QuickJS stops it with a RangeError before the process stack runs out.
function down(n) { return down(n + 1) + 1; }
let deep = null;
try { down(0); } catch (e) { deep = e; }
check("recursion: RangeError, process alive", deep instanceof RangeError && deep.message === "stack overflow", deep);
function depth(n) { try { return depth(n + 1); } catch (e) { return n; } }
const maxDepth = depth(0);
check("recursion: engine still works after it", maxDepth > 1000 && [1, 2, 3].map((x) => x * 2).join() === "2,4,6", maxDepth);
console.log("info deepest JS recursion: " + maxDepth + " calls");

console.log("selftest: " + passed + "/" + (passed + failed) + " ok" + (failed ? ", " + failed + " FAILED" : ""));

// Benchmark (TCG under QEMU is many times slower than real hardware; see docs/design/processos.md).
function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
let t0 = performance.now();
const f27 = fib(27);
let t1 = performance.now();
console.log("bench fib(27) = " + f27 + " in " + Math.round(t1 - t0) + " ms");
t0 = performance.now();
let acc = 0;
for (let i = 0; i < 5000000; i++) acc += i;
t1 = performance.now();
console.log("bench loop 5e6 sums = " + acc + " in " + Math.round(t1 - t0) + " ms");
t0 = performance.now();
const objs = [];
for (let i = 0; i < 100000; i++) objs.push({ i, s: "n" + i });
objs.sort((x, y) => y.i - x.i);
const joined = objs.slice(0, 1000).map((o) => o.s).join("").length;
t1 = performance.now();
console.log("bench 100k objects + sort = " + joined + " in " + Math.round(t1 - t0) + " ms");
if (failed) throw new Error(failed + " selftest checks failed");
