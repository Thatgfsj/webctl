// webctl 最小 DOM/BOM 垫片（ADR-0005 第 4 条 / 议题 #128）。
//
// **它跑在沙箱里，是纯 JS**：不注册任何 Rust 宿主函数，因此不改变 #127 的能力面——
// 沙箱里依旧没有 fetch / require / process / 定时器 / 文件系统。
//
// 实现的子集（够"把服务端渲染缺的内容从脚本里取出来"即可，不做布局）：
//   解析（标签/属性/文本/注释）、document.write、createElement/appendChild、
//   setAttribute/getAttribute、textContent、innerHTML（读+写）、querySelector(All)（#id/.class/tag）、
//   getElementsByTagName、outerHTML 序列化。
//
// 明确**不提供**：setTimeout/setInterval（定时器永不触发是"静默省略"，不如直接不提供）、
//   网络、存储、事件循环的真实语义（addEventListener 是空实现）、layout/样式。
(function (initialHtml) {
  var VOID = { area: 1, base: 1, br: 1, col: 1, embed: 1, hr: 1, img: 1, input: 1, link: 1, meta: 1, param: 1, source: 1, track: 1, wbr: 1 };

  function Node(tag) { this.tagName = tag; this.children = []; this.attrs = {}; }
  Node.prototype.appendChild = function (child) { this.children.push(child); return child; };
  Node.prototype.setAttribute = function (k, v) { this.attrs[String(k).toLowerCase()] = String(v); };
  Node.prototype.getAttribute = function (k) { var key = String(k).toLowerCase(); return Object.prototype.hasOwnProperty.call(this.attrs, key) ? this.attrs[key] : null; };
  Node.prototype.removeAttribute = function (k) { delete this.attrs[String(k).toLowerCase()]; };

  function decode(s) {
    return String(s).replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, "\"").replace(/&#39;/g, "'").replace(/&amp;/g, "&");
  }
  function esc(s) { return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;"); }

  // 极简解析：不追求 HTML5 错误恢复，够解析常规页面与 document.write 的片段。
  function parse(html, parent) {
    var i = 0, n = html.length, stack = [parent];
    function top() { return stack[stack.length - 1]; }
    while (i < n) {
      var lt = html.indexOf("<", i);
      if (lt < 0) { top().children.push(decode(html.slice(i))); break; }
      if (lt > i) top().children.push(decode(html.slice(i, lt)));
      if (html.slice(lt, lt + 4) === "<!--") { var cend = html.indexOf("-->", lt + 4); i = cend < 0 ? n : cend + 3; continue; }
      if (html.charAt(lt + 1) === "!" || html.charAt(lt + 1) === "?") { var bang = html.indexOf(">", lt); i = bang < 0 ? n : bang + 1; continue; }
      var gt = html.indexOf(">", lt);
      if (gt < 0) { top().children.push(decode(html.slice(lt))); break; }
      var raw = html.slice(lt + 1, gt);
      i = gt + 1;
      if (raw.charAt(0) === "/") {
        var closing = raw.slice(1).trim().toLowerCase();
        for (var j = stack.length - 1; j > 0; j--) { if (stack[j].tagName === closing) { stack.length = j; break; } }
        continue;
      }
      var selfClose = raw.charAt(raw.length - 1) === "/";
      if (selfClose) raw = raw.slice(0, -1);
      var tag = raw.split(/[\s\/]/)[0].toLowerCase();
      if (!tag) continue;
      var node = defineText(new Node(tag));
      var attrText = raw.slice(tag.length);
      var re = /([a-zA-Z_:][-a-zA-Z0-9_:.]*)(?:\s*=\s*("[^"]*"|'[^']*'|[^\s"'>]+))?/g, m;
      while ((m = re.exec(attrText)) !== null) {
        var value = m[2];
        if (value === undefined) value = "";
        else if (value.charAt(0) === '"' || value.charAt(0) === "'") value = value.slice(1, -1);
        node.setAttribute(m[1], decode(value));
      }
      top().appendChild(node);
      if (!selfClose && !VOID[tag]) stack.push(node);
    }
    return parent;
  }

  function walk(node, fn) {
    if (typeof node === "string") return;
    for (var i = 0; i < node.children.length; i++) {
      var child = node.children[i];
      if (typeof child === "string") continue;
      if (fn(child) === true) return;
      walk(child, fn);
    }
  }
  function collect(node, out) {
    if (typeof node === "string") { out.push(node); return out; }
    for (var i = 0; i < node.children.length; i++) collect(node.children[i], out);
    return out;
  }
  function matches(node, selector) {
    if (selector.charAt(0) === "#") return node.getAttribute("id") === selector.slice(1);
    if (selector.charAt(0) === ".") {
      var cls = node.getAttribute("class");
      return !!cls && (" " + cls + " ").indexOf(" " + selector.slice(1) + " ") >= 0;
    }
    return node.tagName === selector.toLowerCase();
  }
  function find(root, selector, all) {
    var hits = [];
    walk(root, function (node) {
      if (matches(node, selector)) { hits.push(node); if (!all) return true; }
      return false;
    });
    return all ? hits : hits[0] || null;
  }

  function serialize(node) {
    if (typeof node === "string") return esc(node);
    var out = "<" + node.tagName;
    for (var k in node.attrs) { if (Object.prototype.hasOwnProperty.call(node.attrs, k)) out += " " + k + "=\"" + esc(node.attrs[k]) + "\""; }
    if (VOID[node.tagName]) return out + ">";
    out += ">";
    for (var i = 0; i < node.children.length; i++) out += serialize(node.children[i]);
    return out + "</" + node.tagName + ">";
  }

  function defineText(node) {
    Object.defineProperty(node, "textContent", {
      get: function () { return collect(node, []).join(""); },
      set: function (value) { node.children = [String(value)]; },
      configurable: true,
    });
    Object.defineProperty(node, "innerHTML", {
      get: function () { var out = ""; for (var i = 0; i < node.children.length; i++) out += serialize(node.children[i]); return out; },
      set: function (value) { node.children = []; parse(String(value), node); },
      configurable: true,
    });
    Object.defineProperty(node, "outerHTML", { get: function () { return serialize(node); }, configurable: true });
    node.querySelector = function (selector) { return find(node, selector, false); };
    node.querySelectorAll = function (selector) { return find(node, selector, true); };
    node.getElementsByTagName = function (tag) { return find(node, tag, true); };
    node.addEventListener = function () {};
    node.removeEventListener = function () {};
    node.insertBefore = function (child) { return node.appendChild(child); };
    node.removeChild = function (child) {
      var index = node.children.indexOf(child);
      if (index >= 0) node.children.splice(index, 1);
      return child;
    };
    return node;
  }

  // 先解析成"片段"，再采纳其中的 <html> 作为 documentElement——否则会出现双层 <html>。
  var fragment = new Node("#fragment");
  parse(initialHtml, fragment);
  var documentElement = find(fragment, "html", false) || defineText(fragment.appendChild(new Node("html")));
  var head = find(documentElement, "head", false) || defineText(documentElement.appendChild(new Node("head")));
  var body = find(documentElement, "body", false) || defineText(documentElement.appendChild(new Node("body")));

  var doc = {
    documentElement: documentElement,
    head: head,
    body: body,
    write: function () {
      var html = "";
      for (var i = 0; i < arguments.length; i++) html += String(arguments[i]);
      parse(html, body);
    },
    writeln: function () { doc.write.apply(null, arguments); },
    createElement: function (tag) { return defineText(new Node(String(tag).toLowerCase())); },
    createTextNode: function (text) { return String(text); },
    getElementById: function (id) { return find(documentElement, "#" + id, false); },
    querySelector: function (selector) { return find(documentElement, selector, false); },
    querySelectorAll: function (selector) { return find(documentElement, selector, true); },
    getElementsByTagName: function (tag) { return find(documentElement, tag, true); },
    addEventListener: function () {},
    removeEventListener: function () {},
  };
  Object.defineProperty(doc, "title", {
    get: function () { var t = find(documentElement, "title", false); return t ? t.textContent : ""; },
    set: function (value) { var t = find(documentElement, "title", false); if (!t) { t = defineText(head.appendChild(new Node("title"))); } t.textContent = String(value); },
    configurable: true,
  });

  globalThis.document = doc;
  globalThis.window = globalThis;
  globalThis.location = { href: "about:blank", protocol: "about:", host: "", hostname: "", pathname: "/", search: "", hash: "" };
  globalThis.navigator = { userAgent: "webctl-minidom/0.1" };
  globalThis.__webctl_serialize = function () { return serialize(documentElement); };
  globalThis.__webctl_ready = 1;
})(__WEBCTL_INITIAL_HTML__);
