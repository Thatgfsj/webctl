// 手写 CDP 探针：验证不依赖 Playwright 能否驱动 Edge
// 只做「协议层」验证：连接、扁平会话、Page/Runtime/DOM/Input/Accessibility/Network/Emulation
const port = process.argv[2] || '9333';
const base = `http://127.0.0.1:${port}`;

const ver = await (await fetch(`${base}/json/version`)).json();
const out = { browser: ver.Browser, protocol: ver['Protocol-Version'] };

const ws = new WebSocket(ver.webSocketDebuggerUrl);
await new Promise((res, rej) => { ws.onopen = res; ws.onerror = () => rej(new Error('ws connect failed')); });

let seq = 0;
const pending = new Map();
const events = [];
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) {
    const { res, rej } = pending.get(m.id);
    pending.delete(m.id);
    m.error ? rej(new Error(`${m.method || ''} ${JSON.stringify(m.error)}`)) : res(m.result);
  } else if (m.method) {
    events.push(m.method);
  }
};

function send(method, params = {}, sessionId) {
  const id = ++seq;
  return new Promise((res, rej) => {
    pending.set(id, { res, rej });
    ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    setTimeout(() => { if (pending.has(id)) { pending.delete(id); rej(new Error(`timeout ${method}`)); } }, 15000);
  });
}

const html = `<!doctype html><html lang="zh"><head><meta charset="utf-8"><title>webctl probe</title></head>
<body><h1 id="h">Hello webctl</h1>
<input id="inp" aria-label="姓名">
<button id="btn" aria-label="提交按钮"
  onclick="window.__n=(window.__n||0)+1;document.getElementById('h').textContent='已点击'+window.__n">提交</button>
<ul><li>alpha</li><li>beta</li></ul>
<script>fetch('data:application/json,{"ok":true}');</script></body></html>`;
const dataUrl = 'data:text/html;charset=utf-8,' + encodeURIComponent(html);

const t = await send('Target.createTarget', { url: 'about:blank' });
out.createTarget = t.targetId ? 'ok' : 'fail';

const attached = await send('Target.attachToTarget', { targetId: t.targetId, flatten: true });
const sid = attached.sessionId;
out.flattenSession = !!sid;

await send('Page.enable', {}, sid);
await send('Runtime.enable', {}, sid);
await send('Network.enable', {}, sid);
await send('DOM.enable', {}, sid);
await send('Accessibility.enable', {}, sid);

out.navigate = (await send('Page.navigate', { url: dataUrl }, sid)).frameId ? 'ok' : 'fail';
await new Promise((r) => setTimeout(r, 600));

const evalJs = async (expr) => (await send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true }, sid)).result.value;
out.docTitle = await evalJs('document.title');
out.uaHead = (await evalJs('navigator.userAgent')).slice(0, 40);
out.webdriverFlag = await evalJs('navigator.webdriver');

// DOM + BoxModel -> 真实鼠标点击坐标
const doc = await send('DOM.getDocument', { depth: -1 }, sid);
const btn = await send('DOM.querySelector', { nodeId: doc.root.nodeId, selector: '#btn' }, sid);
const box = await send('DOM.getBoxModel', { nodeId: btn.nodeId }, sid);
const q = box.model.content;
const x = (q[0] + q[2]) / 2;
const y = (q[1] + q[5]) / 2;
for (const type of ['mousePressed', 'mouseReleased']) {
  await send('Input.dispatchMouseEvent', { type, x, y, button: 'left', clickCount: 1 }, sid);
}
await new Promise((r) => setTimeout(r, 400));
out.clickResult = await evalJs("document.getElementById('h').textContent");

// 中文输入必须走 insertText，不能走 keyEvent
const inp = await send('DOM.querySelector', { nodeId: doc.root.nodeId, selector: '#inp' }, sid);
await send('DOM.focus', { nodeId: inp.nodeId }, sid);
await send('Input.insertText', { text: '中文输入测试' }, sid);
out.typedValue = await evalJs("document.getElementById('inp').value");

// 可访问性树 -> 给 AI 用的稳定 ref 来源
const ax = await send('Accessibility.getFullAXTree', {}, sid);
out.axNodeCount = ax.nodes.length;
out.axButtonName = (ax.nodes.find((n) => n.role?.value === 'button')?.name?.value) ?? null;

// 网络抓包
await send('Page.reload', {}, sid);
await new Promise((r) => setTimeout(r, 800));
out.networkEvents = events.filter((e) => e.startsWith('Network.')).length;

// 截图 + 设备仿真
const shot = await send('Page.captureScreenshot', { format: 'png' }, sid);
out.screenshotKB = Math.round(Buffer.from(shot.data, 'base64').length / 1024);
await send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true }, sid);
out.emulateMobile = await evalJs('window.innerWidth + "x" + window.innerHeight');

await send('Target.closeTarget', { targetId: t.targetId });
console.log(JSON.stringify(out, null, 2));
ws.close();
