(() => {
  try {
    const refs = Object.create(null), nonce = Math.random().toString(36).slice(2);
    const elements = [], texts = [], seen = new Set(), unavailableFrames = [];
    const previous = window.__pipiBrowserState;
    if (previous?.observers) for (const observer of previous.observers) observer.disconnect();
    let omitted = false;
    const visible = el => {
      const r = el.getBoundingClientRect(), s = el.ownerDocument.defaultView.getComputedStyle(el);
      return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
    };
    const name = el => (el.getAttribute('aria-label') ||
      Array.from(el.labels || []).map(label => label.innerText).join(' ') ||
      (el.getAttribute('aria-labelledby') || '').split(/\s+/).map(id => el.ownerDocument.getElementById(id)?.innerText || '').join(' ').trim() ||
      el.innerText || el.getAttribute('placeholder') || el.getAttribute('title') || '').trim().slice(0, 200);
    const signature = el => JSON.stringify([el.tagName, el.getAttribute('role'), el.getAttribute('type'), name(el),
      'value' in el && !['password', 'hidden', 'file'].includes(el.type) ? el.value : null,
      'checked' in el ? el.checked : null]);
    const state = { refs, url: location.href, visible, signature, dirty: false, observers: [] };
    function visit(root) {
      if (!root || seen.has(root)) return;
      if (seen.size >= 256) { omitted = true; return; }
      seen.add(root);
      const observer = new MutationObserver(() => { state.dirty = true; });
      observer.observe(root, { subtree: true, childList: true, attributes: true, characterData: true });
      state.observers.push(observer);
      if (root.body && root.body.innerText) texts.push(root.body.innerText.slice(0, 24000));
      for (const el of root.querySelectorAll('*')) {
        if (el.shadowRoot) visit(el.shadowRoot);
        if (el.tagName === 'IFRAME' && visible(el)) {
          let child = null;
          try { child = el.contentDocument; } catch (_) {}
          if (child) visit(child);
          else if (unavailableFrames.length < 50) unavailableFrames.push({ title: (el.getAttribute('title') || '').slice(0, 200), reason: 'Frame content is unavailable, possibly cross-origin or still loading.' });
          else omitted = true;
        }
        if (!el.matches('a,button,input,textarea,select,[role=button],[role=link],[role=checkbox],[role=radio],[role=textbox],[contenteditable=true]')) continue;
        if (!visible(el) || el.type === 'password' || el.type === 'hidden') continue;
        if (elements.length >= 200) { omitted = true; continue; }
        const id = 'e' + (elements.length + 1) + '-' + nonce;
        refs[id] = { el, signature: signature(el) };
        const info = { id, tag: el.tagName.toLowerCase(), role: el.getAttribute('role'), text: name(el), disabled: !!el.disabled || el.getAttribute('aria-disabled') === 'true' };
        if ((el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') && !['password', 'hidden', 'file'].includes(el.type)) info.value = String(el.value || '').slice(0, 1000);
        if ('checked' in el) info.checked = !!el.checked;
        if (el.tagName === 'SELECT') info.options = Array.from(el.options).slice(0, 50).map(option => ({ value: option.value.slice(0, 200), text: option.text.slice(0, 200), selected: option.selected, disabled: option.disabled }));
        elements.push(info);
      }
    }
    visit(document);
    window.__pipiBrowserState = state;
    const text = texts.join('\n').slice(0, 24000);
    return JSON.stringify({ ok: true, title: document.title.slice(0, 300), url: location.href, text, elements, unavailableFrames, truncated: omitted || texts.join('\n').length > text.length });
  } catch (e) { return JSON.stringify({ ok: false, error: String(e) }); }
})()
