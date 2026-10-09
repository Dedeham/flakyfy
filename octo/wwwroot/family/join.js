/*
 * The page a join link or QR code opens: /family/join#u=<username>&c=<code> adds a device,
 * /family/join#invite=<token> sets up a new member's account first. The secrets stay in the
 * part after #, which browsers never send to a server. A `home=` there names the server's
 * home address, which the Octo app may use on the home network.
 *
 * An invite walks through three steps, one card each: 1 your name and password, 2 get the
 * app, 3 done. On Android the Octo app is opened straight away through an intent link that
 * comes back here when the app is not installed; on a computer the page offers Open in Octo.
 */
(function () {
  'use strict';

  var F = window.OctoFamily;
  var el = F.el;
  var root = document.getElementById('join');
  var server = location.origin;
  document.getElementById('host').textContent = location.host;

  var STEPS = ['Your name and password', 'Get the app', 'Done'];

  function params() {
    return new URLSearchParams(location.hash.replace(/^#/, ''));
  }

  function card() {
    var node = el('section', { class: 'info-card fam-card' });
    for (var i = 0; i < arguments.length; i++) F.append(node, arguments[i]);
    return node;
  }

  function heading(text, lede) {
    return el('header', { class: 'page-header fam-join-head' },
      el('div', null, el('h1', { tabindex: '-1' }, text), lede ? el('p', null, lede) : null));
  }

  function progress(step) {
    return el('ol', { class: 'fam-progress', 'aria-label': 'Step ' + step + ' of ' + STEPS.length },
      STEPS.map(function (words, i) {
        var n = i + 1;
        return el('li', { class: (n <= step ? 'on' : '') + (n === step ? ' now' : ''), 'aria-current': n === step ? 'step' : null },
          el('span', null, n + ' ' + words));
      }));
  }

  function show(parts) {
    F.clear(root);
    F.append(root, parts);
    window.scrollTo(0, 0);
    var h1 = root.querySelector('h1');
    if (h1) h1.focus({ preventScroll: true });
  }

  function familyLink(words) {
    return el('p', { class: 'fam-help fam-gap' }, words || 'Your devices, plan and audio quality are on ', el('a', { href: '/family/' }, 'your Family page'), '.');
  }

  // ── A code: add this device ──────────────────────────────────────────────────────────
  function pageUrl(username, code, home, back) {
    return server + '/family/join#u=' + encodeURIComponent(username) + '&c=' + encodeURIComponent(code) +
      (home ? '&home=' + encodeURIComponent(home) : '') + (back ? '&back=1' : '');
  }

  function openOctoButton(username, code, home) {
    var platform = F.platform();
    if (platform === 'android') {
      return el('a', { class: 'btn btn-primary fam-big', href: F.intentLink(server, username, code, pageUrl(username, code, home, true), home) }, 'Open the Octo app');
    }
    if (platform === 'desktop') {
      return el('a', { class: 'btn btn-primary fam-big', href: F.octoLink(server, username, code, home) }, 'Open in Octo');
    }
    return null;
  }

  function codeCard(username, code, home) {
    var open = openOctoButton(username, code, home);
    var platform = F.platform();
    var groups = String(code).match(/.{1,3}/g) || [];
    return card(
      el('h2', { class: 'info-card-title' }, 'With the Octo app'),
      el('p', { class: 'fam-card-desc' }, open ? 'Press ' + open.textContent + '. If nothing happens, Octo is not installed yet: see Get Octo below.' : 'Open Octo and choose Join with a family code.'),
      open ? el('div', { class: 'fam-actions' }, open) : null,
      el('div', { class: 'fam-pair fam-gap' },
        platform === 'desktop' ? el('figure', { class: 'fam-qr-panel' }, F.qrCard(pageUrl(username, code, home, false), 'QR code to add a phone'), el('figcaption', null, 'To add a phone, scan this with its camera.')) : null,
        el('div', { class: 'fam-pair-info' },
          el('section', { class: 'fam-way' },
            el('h3', { class: 'fam-way-title' }, 'Or type it in Octo: ', el('strong', null, 'Join with a family code')),
            F.table(
              F.row('Server', server, { copy: 'Copy the server' }),
              F.row('Username', username, { copy: 'Copy the username' }),
              F.row('Code', code, { big: true, value: el('span', { class: 'fam-bigcode' }, groups.map(function (g) { return el('span', null, g); })), copy: 'Copy the code' }))))));
  }

  function otherAppCard(username, code, after) {
    var body = el('div', { class: 'fam-form' });
    var name = el('input', { class: 'set-input', id: 'app-name', placeholder: 'Symfonium on my phone', autocomplete: 'off' });
    var error = el('p', { class: 'field-error', role: 'alert' });
    var button = el('button', { class: 'btn btn-primary fam-big', type: 'submit' }, 'Make an app password');
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
      if (after) body.appendChild(el('div', { class: 'fam-actions' }, after));
    } },
    el('label', { class: 'fam-label', for: 'app-name' }, 'A name for it', name, el('span', { class: 'fam-help' }, 'So you can tell your devices apart later.')),
    error,
    el('div', null, button)));
    return card(
      el('h2', { class: 'info-card-title' }, 'With another app'),
      el('p', { class: 'fam-card-desc' }, 'Any Subsonic app works with Octo: Symfonium, Feishin, Amperfy, Substreamer, Tempo and more. Making an app password uses up this code.'),
      el('div', { class: 'fam-gap' }, body));
  }

  function renderCode(username, code, home, lede, step, who) {
    var done = step ? el('button', { class: 'btn btn-primary fam-big', type: 'button', onclick: function () { renderDone(who || username); } }, 'I’m signed in') : null;
    show([
      step ? progress(step) : null,
      heading('Add this device', lede || 'This adds a device to ' + username + '’s Octo account.'),
      codeCard(username, code, home),
      card(el('h2', { class: 'info-card-title' }, 'Get Octo'), el('div', { class: 'fam-gap' }, F.getOcto())),
      otherAppCard(username, code, step ? el('button', { class: 'btn btn-primary fam-big', type: 'button', onclick: function () { renderDone(who || username); } }, 'I’m signed in') : null),
      done ? el('div', { class: 'fam-actions' }, done) : null,
      familyLink()
    ]);
  }

  // ── An invite: set up the account, then a first device ───────────────────────────────
  function renderInvite(token) {
    var name = el('input', { class: 'set-input', id: 'name', autocomplete: 'name', placeholder: 'Alex' });
    var pass = el('input', { class: 'set-input', id: 'password', type: 'password', autocomplete: 'new-password', minlength: 8, required: true });
    var again = el('input', { class: 'set-input', id: 'password-again', type: 'password', autocomplete: 'new-password', minlength: 8, required: true });
    var error = el('p', { class: 'field-error', role: 'alert' });
    var button = el('button', { class: 'btn btn-primary btn-block fam-big', type: 'submit' }, 'Continue');
    var form = el('form', { class: 'fam-form', onsubmit: async function (e) {
      e.preventDefault();
      error.textContent = '';
      if (pass.value !== again.value) {
        error.textContent = 'The two passwords are not the same.';
        again.focus();
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
      progress(1),
      heading('Welcome to the family', 'Someone invited you to listen with them on Octo. Set up your account, then add your first device.'),
      card(form)
    ]);
  }

  function renderFirstDevice(username) {
    var pick = async function (kind) {
      var reply = await F.api('POST', '/me/devices', { name: kind === 'OctoApp' ? '' : 'My first app', kind: kind });
      if (!reply.ok) return F.toast(F.errorOf(reply), true);
      var added = reply.data;
      if (kind === 'OctoApp') {
        var home = added.servers && added.servers.anywhere && added.servers.home !== added.servers.anywhere ? added.servers.home : '';
        renderCode(added.username, added.pairCode, home, 'You are in, ' + username + '. Now add this device: it takes a minute.', 2, username);
        return;
      }
      show([
        progress(2),
        heading('Sign in to your app', 'Type these into your app, or copy them across. The app password is shown once.'),
        card(F.otherApp(added.server, added.username, added.appPassword)),
        el('div', { class: 'fam-actions' }, el('button', { class: 'btn btn-primary fam-big', type: 'button', onclick: function () { renderDone(username); } }, 'I’m signed in'))
      ]);
    };
    show([
      progress(2),
      heading('Get the app', 'You are in, ' + username + '. Which app will you listen with?'),
      card(el('div', { class: 'fam-choice' },
        el('button', { class: 'fam-option', type: 'button', onclick: function () { pick('OctoApp'); } },
          F.icon('phone'),
          el('span', { class: 'fam-option-title' }, 'The Octo app'),
          el('span', { class: 'fam-option-sub' }, 'Made for this server: requests, family playlists and your audio quality.')),
        el('button', { class: 'fam-option', type: 'button', onclick: function () { pick('SubsonicApp'); } },
          F.icon('stack'),
          el('span', { class: 'fam-option-title' }, 'Another app'),
          el('span', { class: 'fam-option-sub' }, 'Symfonium, Feishin, Amperfy, Substreamer, Tempo or any Subsonic app.')))),
      el('p', { class: 'fam-help fam-gap' }, 'Or go to ', el('a', { href: '/family/' }, 'your Family page'), ' and add devices later.')
    ]);
  }

  function renderDone(username) {
    show([
      progress(3),
      heading('You are all set', 'Welcome, ' + username + '. Your music is ready on your device.'),
      card(
        el('div', { class: 'fam-empty' },
          el('span', { class: 'fam-empty-ico', 'aria-hidden': 'true' }, F.icon('check')),
          el('p', null, 'Heart songs in your app to save them. Your devices, plan and audio quality are on your Family page.'),
          el('a', { class: 'btn btn-primary fam-big', href: '/family/' }, 'Open my Family page')))
    ]);
  }

  // ── Start ────────────────────────────────────────────────────────────────────────────
  function start() {
    var p = params();
    var invite = p.get('invite');
    var username = p.get('u');
    var code = p.get('c');
    var home = p.get('home') || '';
    if (invite) return renderInvite(invite);
    if (username && code) {
      if (F.platform() === 'android' && !p.get('back')) {
        // Opens the app at once; without it, Android comes back to this page with back=1.
        location.href = F.intentLink(server, username, code, pageUrl(username, code, home, true), home);
      }
      return renderCode(username, code, home);
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
      card(el('div', { class: 'fam-actions', style: 'margin:0' }, el('a', { class: 'btn btn-primary fam-big', href: '/family/' }, 'Open the Family page')))
    ]);
  }

  window.addEventListener('hashchange', start);
  start();
})();
