/*
 * What the Family page and the join page share: calls to /api/family, building elements, the
 * sheet and the toast, copy buttons, sizes and times in words, QR cards, and the steps for
 * signing in to other music apps.
 */
(function () {
  'use strict';

  var F = (window.OctoFamily = {});

  // ── Calls ────────────────────────────────────────────────────────────────────────────
  // Every write carries X-Octo-Family: 1. Answers { ok, status, data } and never throws.
  F.api = async function (method, path, body) {
    var options = { method: method, credentials: 'same-origin', headers: {} };
    if (method !== 'GET') {
      options.headers['X-Octo-Family'] = '1';
    }
    if (body !== undefined) {
      options.headers['Content-Type'] = 'application/json';
      options.body = JSON.stringify(body);
    }
    try {
      var res = await fetch('/api/family' + path, options);
      var data = null;
      var text = await res.text();
      if (text) {
        try { data = JSON.parse(text); } catch (e) { data = { error: text }; }
      }
      return { ok: res.ok, status: res.status, data: data };
    } catch (e) {
      return { ok: false, status: 0, data: { error: 'Octo did not answer. Check your connection and try again.' } };
    }
  };

  F.errorOf = function (reply) {
    return (reply && reply.data && reply.data.error) || 'Something went wrong. Try again.';
  };

  // ── Elements ─────────────────────────────────────────────────────────────────────────
  // el('div', { class: 'x', onclick: fn }, child, 'text', [more]). Text is always text.
  F.el = function (tag, props) {
    var node = document.createElement(tag);
    if (props) {
      Object.keys(props).forEach(function (key) {
        var value = props[key];
        if (value === null || value === undefined || value === false) return;
        if (key.indexOf('on') === 0 && typeof value === 'function') {
          node.addEventListener(key.slice(2), value);
        } else if (key === 'class') {
          node.className = value;
        } else if (key === 'text') {
          node.textContent = value;
        } else if (key === 'value') {
          node.value = value;
        } else if (key === 'checked') {
          node.checked = !!value;
        } else {
          node.setAttribute(key, value === true ? '' : String(value));
        }
      });
    }
    for (var i = 2; i < arguments.length; i++) F.append(node, arguments[i]);
    return node;
  };

  F.append = function (node, child) {
    if (child === null || child === undefined || child === false) return;
    if (Array.isArray(child)) {
      child.forEach(function (c) { F.append(node, c); });
    } else if (typeof child === 'string' || typeof child === 'number') {
      node.appendChild(document.createTextNode(String(child)));
    } else {
      node.appendChild(child);
    }
  };

  F.clear = function (node) {
    while (node.firstChild) node.removeChild(node.firstChild);
    return node;
  };

  // ── Toast and sheet ──────────────────────────────────────────────────────────────────
  var toastTimer = null;
  F.toast = function (message, bad) {
    var toast = document.getElementById('toast');
    if (!toast) return;
    toast.textContent = message;
    toast.className = 'fam-toast show' + (bad ? ' bad' : '');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(function () { toast.className = 'fam-toast' + (bad ? ' bad' : ''); }, 3600);
  };

  F.openSheet = function (title, body) {
    var sheet = document.getElementById('sheet');
    F.clear(sheet);
    var close = F.el('button', { class: 'fam-close', type: 'button', 'aria-label': 'Close', onclick: F.closeSheet }, '×');
    sheet.appendChild(
      F.el('div', { class: 'fam-sheet-body' },
        F.el('div', { class: 'fam-sheet-head' }, F.el('h2', { id: 'sheet-title' }, title), close),
        body)
    );
    if (!sheet.open) sheet.showModal();
    return sheet;
  };

  F.closeSheet = function () {
    var sheet = document.getElementById('sheet');
    if (sheet && sheet.open) sheet.close();
  };

  // ── Copying ──────────────────────────────────────────────────────────────────────────
  F.copy = async function (text) {
    try {
      await navigator.clipboard.writeText(text);
    } catch (e) {
      var area = F.el('textarea', { class: 'visually-hidden' });
      area.value = text;
      document.body.appendChild(area);
      area.select();
      try { document.execCommand('copy'); } catch (ignored) { /* nothing more to try */ }
      area.remove();
    }
    F.toast('Copied');
  };

  F.copyButton = function (text, label) {
    return F.el('button', { class: 'fam-btn small', type: 'button', onclick: function () { F.copy(text); } }, label || 'Copy');
  };

  // A labeled value with a Copy button.
  F.secret = function (label, value) {
    return F.el('div', { class: 'fam-kv' },
      F.el('div', { class: 'fam-kv-label' }, label),
      F.el('div', { class: 'fam-secret' }, F.el('span', null, value), F.copyButton(value)));
  };

  // ── Words ────────────────────────────────────────────────────────────────────────────
  F.bytes = function (n) {
    n = Number(n) || 0;
    if (n >= 1e9) return (n / 1e9).toFixed(n >= 1e10 ? 0 : 1) + ' GB';
    if (n >= 1e6) return Math.round(n / 1e6) + ' MB';
    if (n >= 1e3) return Math.round(n / 1e3) + ' KB';
    return n + ' bytes';
  };

  F.ago = function (iso) {
    if (!iso) return 'Not seen yet';
    var then = new Date(iso).getTime();
    if (isNaN(then)) return '';
    var seconds = Math.max(0, Math.round((Date.now() - then) / 1000));
    if (seconds < 60) return 'Just now';
    var minutes = Math.round(seconds / 60);
    if (minutes < 60) return minutes === 1 ? 'A minute ago' : minutes + ' minutes ago';
    var hours = Math.round(minutes / 60);
    if (hours < 24) return hours === 1 ? 'An hour ago' : hours + ' hours ago';
    var days = Math.round(hours / 24);
    if (days === 1) return 'Yesterday';
    if (days < 30) return days + ' days ago';
    return new Date(iso).toLocaleDateString();
  };

  F.until = function (iso) {
    var left = Math.round((new Date(iso).getTime() - Date.now()) / 60000);
    if (left <= 0) return 'This code has run out. Make a new one.';
    return left === 1 ? 'Works for 1 more minute.' : 'Works for ' + left + ' more minutes.';
  };

  F.initials = function (name) {
    var parts = String(name || '?').trim().split(/\s+/);
    return (parts[0].charAt(0) + (parts.length > 1 ? parts[parts.length - 1].charAt(0) : '')).toUpperCase();
  };

  // ── Links ────────────────────────────────────────────────────────────────────────────
  F.joinLink = function (server, username, code) {
    return server + '/family/join#u=' + encodeURIComponent(username) + '&c=' + encodeURIComponent(code);
  };

  F.inviteLink = function (server, token) {
    return server + '/family/join#invite=' + encodeURIComponent(token);
  };

  F.octoLink = function (server, username, code) {
    return 'octo://join?server=' + encodeURIComponent(server) + '&username=' + encodeURIComponent(username) +
      '&code=' + encodeURIComponent(code);
  };

  F.intentLink = function (server, username, code, fallback) {
    return 'intent://join?server=' + encodeURIComponent(server) + '&username=' + encodeURIComponent(username) +
      '&code=' + encodeURIComponent(code) + '#Intent;scheme=octo;package=app.winters.octo;S.browser_fallback_url=' +
      encodeURIComponent(fallback) + ';end';
  };

  F.platform = function () {
    var ua = navigator.userAgent || '';
    if (/android/i.test(ua)) return 'android';
    if (/iphone|ipad|ipod/i.test(ua) || (/macintosh/i.test(ua) && navigator.maxTouchPoints > 1)) return 'ios';
    return 'desktop';
  };

  // A QR code of `link` on a white card.
  F.qrCard = function (link, label) {
    var wrap = F.el('div', { class: 'fam-qr' });
    try {
      wrap.appendChild(window.OctoQr.svg(link, label));
    } catch (e) {
      wrap.appendChild(F.el('p', { class: 'fam-small' }, 'This link is too long for a QR code.'));
    }
    return wrap;
  };

  // ── Other apps ───────────────────────────────────────────────────────────────────────
  F.APPS = ['Symfonium', 'Feishin', 'Amperfy', 'Substreamer', 'Tempo'];

  F.appSteps = function (app) {
    switch (app) {
      case 'Symfonium':
        return [
          'Open Symfonium and choose Add a media provider.',
          'Choose Subsonic.',
          'Type the server address, your username and the app password below, then Test and Save.'
        ];
      case 'Feishin':
        return [
          'Open Feishin and choose Add server.',
          'Choose Subsonic as the server type.',
          'Type a name, the server address, your username and the app password below, then Add.'
        ];
      case 'Amperfy':
        return [
          'Open Amperfy. On the sign-in screen, type the server address.',
          'Type your username and the app password below.',
          'Choose Subsonic if it asks which kind of server this is, then sign in.'
        ];
      case 'Substreamer':
        return [
          'Open Substreamer and choose Add server.',
          'Type the server address, your username and the app password below.',
          'Save, then wait while it reads the library.'
        ];
      default:
        return [
          'Open Tempo. On the sign-in screen choose Add server.',
          'Type a name, the server address, your username and the app password below.',
          'Save and sign in.'
        ];
    }
  };

  // The server, username and app password with copy buttons, and steps for the app chosen.
  F.otherApp = function (server, username, password) {
    var steps = F.el('ol', { class: 'fam-steps' });
    var picker = F.el('div', { class: 'fam-seg fam-apps', role: 'group', 'aria-label': 'Your app' });
    function show(app) {
      Array.prototype.forEach.call(picker.children, function (b) {
        b.setAttribute('aria-pressed', b.textContent === app ? 'true' : 'false');
      });
      F.clear(steps);
      F.appSteps(app).forEach(function (line) { steps.appendChild(F.el('li', null, line)); });
    }
    F.APPS.forEach(function (app) {
      picker.appendChild(F.el('button', { type: 'button', 'aria-pressed': 'false', onclick: function () { show(app); } }, app));
    });
    show(F.APPS[0]);
    return F.el('div', { class: 'fam-form' },
      F.secret('Server address', server),
      F.secret('Username', username),
      password ? F.secret('App password', password) : null,
      password ? F.el('p', { class: 'fam-help' }, 'This password is shown once. It works only for this app, and you can sign the app out any time from Devices.') : null,
      F.el('div', null, F.el('div', { class: 'fam-kv-label' }, 'Steps for your app'), picker, steps));
  };

  // Where to get Octo, for this device.
  F.FDROID_REPO = 'https://winters27.github.io/octo/fdroid/repo';
  F.RELEASES = 'https://github.com/winters27/octo/releases';

  F.getOcto = function () {
    var platform = F.platform();
    var android = F.el('div', { class: 'fam-form' },
      F.el('div', { class: 'fam-h' }, 'Octo for Android'),
      F.el('ol', { class: 'fam-steps' },
        F.el('li', null, 'In F-Droid, open Settings, then Repositories, and add the address below.'),
        F.el('li', null, 'Search for Octo and install it.')),
      F.secret('F-Droid repository', F.FDROID_REPO),
      F.el('p', { class: 'fam-help' }, 'No F-Droid? ', F.el('a', { href: F.RELEASES, target: '_blank', rel: 'noopener' }, 'Download the app from GitHub'), ' and allow your browser to install it.'));
    var desktop = F.el('div', { class: 'fam-form' },
      F.el('div', { class: 'fam-h' }, 'Octo for Windows, macOS and Linux'),
      F.el('p', { class: 'fam-sub' }, 'Download the installer for your computer, open it, then come back to this page.'),
      F.el('div', null, F.el('a', { class: 'fam-btn primary', href: F.RELEASES, target: '_blank', rel: 'noopener' }, 'Download Octo')));
    if (platform === 'android') return F.el('div', { class: 'fam-form' }, android, desktop);
    if (platform === 'ios') {
      return F.el('div', { class: 'fam-form' },
        F.el('p', { class: 'fam-sub' }, 'Octo has no iPhone download yet, so use another app on this phone. Amperfy and Substreamer both work.'),
        desktop, android);
    }
    return F.el('div', { class: 'fam-form' }, desktop, android);
  };
})();
