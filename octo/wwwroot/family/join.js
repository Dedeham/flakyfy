/*
 * The page a join link or QR code opens: /family/join#u=<username>&c=<code> adds a device,
 * /family/join#invite=<token> sets up a new member's account first. The secrets stay in the
 * part after #, which browsers never send to a server.
 *
 * On Android the Octo app is opened straight away through an intent link that comes back
 * here when the app is not installed. On a computer the page offers Open in Octo. Everywhere
 * it walks through getting Octo or signing in to another app.
 */
(function () {
  'use strict';

  var F = window.OctoFamily;
  var el = F.el;
  var root = document.getElementById('join');
  var server = location.origin;
  document.getElementById('host').textContent = location.host;

  function params() {
    return new URLSearchParams(location.hash.replace(/^#/, ''));
  }

  function card() {
    var node = el('section', { class: 'fam-card' });
    for (var i = 0; i < arguments.length; i++) F.append(node, arguments[i]);
    return node;
  }

  function heading(text, lede) {
    return [el('h1', { class: 'fam-title' }, text), lede ? el('p', { class: 'fam-lede' }, lede) : null];
  }

  function dots(step, of) {
    var row = el('div', { class: 'fam-step-dots', 'aria-hidden': 'true' });
    for (var i = 1; i <= of; i++) row.appendChild(el('span', { class: i <= step ? 'on' : '' }));
    return row;
  }

  function show(parts) {
    F.clear(root);
    F.append(root, parts);
    window.scrollTo(0, 0);
  }

  // ── A code: add this device ──────────────────────────────────────────────────────────
  function pageUrl(username, code, back) {
    return server + '/family/join#u=' + encodeURIComponent(username) + '&c=' + encodeURIComponent(code) + (back ? '&back=1' : '');
  }

  function openOctoButton(username, code) {
    var platform = F.platform();
    if (platform === 'android') {
      return el('a', { class: 'fam-btn primary', href: F.intentLink(server, username, code, pageUrl(username, code, true)) }, 'Open the Octo app');
    }
    if (platform === 'desktop') {
      return el('a', { class: 'fam-btn primary', href: F.octoLink(server, username, code) }, 'Open in Octo');
    }
    return null;
  }

  function codeCard(username, code) {
    var link = pageUrl(username, code, false);
    var open = openOctoButton(username, code);
    var platform = F.platform();
    return card(
      el('h2', { class: 'fam-h' }, 'With the Octo app'),
      el('div', { class: 'fam-qr-wrap' },
        platform === 'desktop' ? F.qrCard(link, 'QR code to add a phone') : null,
        el('div', { class: 'fam-form', style: 'flex:1 1 220px;min-width:0' },
          el('div', { class: 'fam-kv-label' }, 'Code'),
          el('div', { class: 'fam-code' }, code),
          F.secret('Username', username),
          F.secret('Server address', server))),
      el('ol', { class: 'fam-steps', style: 'margin-top:16px' },
        open ? el('li', null, 'Press ' + open.textContent + '. If nothing happens, Octo is not installed yet: see Get Octo below.') : null,
        el('li', null, 'Or open Octo, choose Join with a family code, and type the server address, your username and the code.'),
        platform === 'desktop' ? el('li', null, 'To add a phone, scan this QR code with its camera.') : null),
      open ? el('div', { class: 'fam-actions' }, open) : null);
  }

  function otherAppCard(username, code) {
    var body = el('div', { class: 'fam-form' });
    var name = el('input', { class: 'fam-field', id: 'app-name', placeholder: 'Symfonium on my phone', autocomplete: 'off' });
    var error = el('p', { class: 'fam-error', role: 'alert' });
    var button = el('button', { class: 'fam-btn primary', type: 'submit' }, 'Make an app password');
    body.appendChild(el('p', { class: 'fam-sub' }, 'Any Subsonic app works with Octo. Making an app password uses up this code.'));
    body.appendChild(el('form', { class: 'fam-form', onsubmit: async function (e) {
      e.preventDefault();
      button.disabled = true;
      error.textContent = '';
      var form = new URLSearchParams({
        username: username,
        code: code,
        deviceName: name.value.trim() || 'Another app',
        platform: '',
        kind: 'SubsonicApp',
        f: 'json'
      });
      var answer = null;
      try {
        var res = await fetch('/rest/octoFamilyPair', {
          method: 'POST',
          headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
          body: form.toString()
        });
        answer = (await res.json())['subsonic-response'];
      } catch (ignored) {
        answer = null;
      }
      button.disabled = false;
      if (!answer || answer.status !== 'ok') {
        error.textContent = (answer && answer.error && answer.error.message) || 'Octo did not answer. Check your connection and try again.';
        return;
      }
      var paired = answer.familyPair;
      F.clear(body).appendChild(F.otherApp(server, paired.username, paired.secret));
    } },
    el('label', { class: 'fam-label', for: 'app-name' }, 'A name for it', name),
    error,
    el('div', null, button)));
    return card(el('h2', { class: 'fam-h' }, 'With another app'), body);
  }

  function renderCode(username, code, lede) {
    show([
      heading('Add a device', lede || 'This adds a device to ' + username + '’s Octo account.'),
      codeCard(username, code),
      card(el('h2', { class: 'fam-h' }, 'Get Octo'), F.getOcto()),
      otherAppCard(username, code),
      el('p', { class: 'fam-help', style: 'margin-top:20px' }, 'Your devices and plan are on ', el('a', { href: '/family/' }, 'your Family page'), '.')
    ]);
  }

  // ── An invite: set up the account, then a first device ───────────────────────────────
  function renderInvite(token) {
    var name = el('input', { class: 'fam-field', id: 'name', autocomplete: 'name', placeholder: 'Alex' });
    var pass = el('input', { class: 'fam-field', id: 'password', type: 'password', autocomplete: 'new-password', minlength: 8, required: true });
    var again = el('input', { class: 'fam-field', id: 'password-again', type: 'password', autocomplete: 'new-password', minlength: 8, required: true });
    var error = el('p', { class: 'fam-error', role: 'alert' });
    var button = el('button', { class: 'fam-btn primary block', type: 'submit' }, 'Continue');
    var form = el('form', { class: 'fam-form', onsubmit: async function (e) {
      e.preventDefault();
      error.textContent = '';
      if (pass.value !== again.value) {
        error.textContent = 'The two passwords are not the same.';
        return;
      }
      button.disabled = true;
      var reply = await F.api('POST', '/join', { token: token, password: pass.value, displayName: name.value.trim() || null });
      button.disabled = false;
      if (!reply.ok) {
        error.textContent = F.errorOf(reply);
        return;
      }
      // The link is used up; the address bar should not keep it.
      history.replaceState(null, '', '/family/join#joined');
      renderFirstDevice(name.value.trim() || reply.data.username);
    } },
    el('label', { class: 'fam-label', for: 'name' }, 'Your name', name, el('span', { class: 'fam-help' }, 'How the family sees you.')),
    el('label', { class: 'fam-label', for: 'password' }, 'Choose a password', pass, el('span', { class: 'fam-help' }, 'At least 8 characters. You use it on the Family page; your apps get their own.')),
    el('label', { class: 'fam-label', for: 'password-again' }, 'Type it again', again),
    error,
    button);
    show([
      dots(1, 2),
      heading('Welcome to the family', 'Someone invited you to listen with them on Octo. Set up your account, then add your first device.'),
      card(form)
    ]);
    name.focus();
  }

  function renderFirstDevice(username) {
    var pick = async function (kind) {
      var reply = await F.api('POST', '/me/devices', { name: kind === 'OctoApp' ? '' : 'My first app', kind: kind });
      if (!reply.ok) return F.toast(F.errorOf(reply), true);
      var added = reply.data;
      if (kind === 'OctoApp') {
        renderCode(added.username, added.pairCode, 'You are in, ' + username + '. Now add this device: it takes a minute.');
        return;
      }
      show([
        dots(2, 2),
        heading('Sign in to your app', 'Type these into your app, or copy them across.'),
        card(F.otherApp(added.server, added.username, added.appPassword)),
        el('p', { class: 'fam-help', style: 'margin-top:20px' }, 'More devices, your plan and your audio quality are on ', el('a', { href: '/family/' }, 'your Family page'), '.')
      ]);
    };
    show([
      dots(2, 2),
      heading('Add your first device', 'You are in, ' + username + '. Which app will you listen with?'),
      card(el('div', { class: 'fam-choice' },
        el('button', { class: 'fam-option', type: 'button', onclick: function () { pick('OctoApp'); } },
          el('span', { class: 'fam-option-title' }, 'The Octo app'),
          el('span', { class: 'fam-sub' }, 'Made for this server: requests, family playlists and your audio quality.')),
        el('button', { class: 'fam-option', type: 'button', onclick: function () { pick('SubsonicApp'); } },
          el('span', { class: 'fam-option-title' }, 'Another app'),
          el('span', { class: 'fam-sub' }, 'Symfonium, Feishin, Amperfy, Substreamer, Tempo or any Subsonic app.')))),
      el('p', { class: 'fam-help', style: 'margin-top:20px' }, 'Or go to ', el('a', { href: '/family/' }, 'your Family page'), ' and add devices later.')
    ]);
  }

  // ── Start ────────────────────────────────────────────────────────────────────────────
  function start() {
    var p = params();
    var invite = p.get('invite');
    var username = p.get('u');
    var code = p.get('c');
    if (invite) return renderInvite(invite);
    if (username && code) {
      if (F.platform() === 'android' && !p.get('back')) {
        // Opens the app at once; without it, Android comes back to this page with back=1.
        location.href = F.intentLink(server, username, code, pageUrl(username, code, true));
      }
      return renderCode(username, code);
    }
    if (p.has('joined')) {
      F.api('GET', '/session').then(function (reply) {
        if (reply.ok) renderFirstDevice(reply.data.displayName || reply.data.username);
        else location.replace('/family/');
      });
      return;
    }
    show([
      heading('This link is not complete', 'Ask for a new link, or sign in on the Family page.'),
      card(el('div', { class: 'fam-actions', style: 'margin:0' }, el('a', { class: 'fam-btn primary', href: '/family/' }, 'Open the Family page')))
    ]);
  }

  window.addEventListener('hashchange', start);
  start();
})();
