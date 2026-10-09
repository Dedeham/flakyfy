/*
 * The Family page. Members see their plan, devices, own library, saved songs and requests;
 * people who manage the family also see everyone, the requests inbox, family playlists and
 * the load. Everything goes through /api/family; the page keeps no secrets of its own.
 */
(function () {
  'use strict';

  var F = window.OctoFamily;
  var el = F.el;
  var api = F.api;
  var app = document.getElementById('app');
  var session = null;

  // ── Plain words ──────────────────────────────────────────────────────────────────────
  var ROLE_WORDS = {
    Owner: 'Owner',
    CoAdmin: 'Co-admin',
    Member: 'Member',
    Listener: 'Listener',
    Kid: 'Kid',
    Unmanaged: 'Not in the family'
  };

  var PRESETS = {
    CoAdmin: 'Can manage the family and approve requests, and adds songs straight to their own library.',
    Member: 'Adds songs straight to their own library, with up to 50 GB of space.',
    Listener: 'Saves songs from outside the library and asks for copies, up to 10 a week.',
    Kid: 'Hears clean songs only, and every request needs your approval.'
  };

  // Every preset's abilities, as the server's presets set them.
  var BASE = {
    addToLibrary: 'Direct', requestQuality: 'Best', autoApprove: true, weeklyRequestLimit: 0, streamCap: 0,
    awayCap: 0, away: true, devicesAtOnce: 0, downloadFiles: true, offlineCopies: true, share: true,
    importPlaylists: true, cleanOnly: false, familyPlaylistsEdit: true, manageFamily: true, approveRequests: true,
    storageLimitGb: 0, instantFromFamily: true
  };
  var PRESET_ABILITIES = {
    CoAdmin: Object.assign({}, BASE),
    Member: Object.assign({}, BASE, { manageFamily: false, approveRequests: false, storageLimitGb: 50 }),
    Listener: Object.assign({}, BASE, {
      addToLibrary: 'Request', autoApprove: false, weeklyRequestLimit: 10, downloadFiles: false, share: false,
      manageFamily: false, approveRequests: false, storageLimitGb: 10
    })
  };
  PRESET_ABILITIES.Kid = Object.assign({}, PRESET_ABILITIES.Listener, {
    requestQuality: 'Mp3', weeklyRequestLimit: 5, cleanOnly: true, familyPlaylistsEdit: false, storageLimitGb: 5,
    instantFromFamily: false
  });

  // The abilities a manager edits, in the order and words the sheet shows them.
  var ABILITY_FIELDS = [
    { key: 'addToLibrary', title: 'When they heart a song from outside the library', kind: 'choice',
      options: [['Direct', 'Add it to their library'], ['Request', 'Save it, and they can ask for a copy'], ['SaveOnly', 'Save it only']] },
    { key: 'requestQuality', title: 'Best copy they can ask for', kind: 'choice',
      options: [['Best', 'Best there is'], ['Flac', 'FLAC'], ['Mp3', 'MP3']] },
    { key: 'autoApprove', title: 'Approve their requests at once', help: 'Otherwise each one waits for you.', kind: 'switch' },
    { key: 'weeklyRequestLimit', title: 'Requests a week', help: '0 means no limit.', kind: 'number' },
    { key: 'storageLimitGb', title: 'Space for their own library, in GB', help: '0 means no limit. When it is full, adding a song becomes a request.', kind: 'number' },
    { key: 'instantFromFamily', title: 'Add songs the family already has at once', help: 'No download and no extra space on disk.', kind: 'switch' },
    { key: 'streamCap', title: 'Highest quality at home, in kbps', help: '0 means the original file. Their own quality choice can only go lower.', kind: 'number' },
    { key: 'awayCap', title: 'Highest quality away from home, in kbps', help: '0 means the same as at home.', kind: 'number' },
    { key: 'away', title: 'Listen away from home', kind: 'switch' },
    { key: 'devicesAtOnce', title: 'Devices playing at once', help: '0 means no limit. The newest device plays.', kind: 'number' },
    { key: 'downloadFiles', title: 'Download song files', kind: 'switch' },
    { key: 'offlineCopies', title: 'Keep songs for offline in the Octo apps', kind: 'switch' },
    { key: 'share', title: 'Make share links for people outside the family', kind: 'switch' },
    { key: 'importPlaylists', title: 'Import playlists', kind: 'switch' },
    { key: 'cleanOnly', title: 'Clean songs only', kind: 'switch' },
    { key: 'familyPlaylistsEdit', title: 'Add to family playlists', kind: 'switch' },
    { key: 'manageFamily', title: 'Manage the family', owner: true, kind: 'switch' },
    { key: 'approveRequests', title: 'Approve requests', owner: true, kind: 'switch' }
  ];

  var QUALITIES = [
    ['Original', 'Original'],
    ['High', 'High'],
    ['Standard', 'Standard'],
    ['DataSaver', 'Data saver']
  ];
  var QUALITY_HELP = {
    Original: 'the file as it is.',
    High: 'about 256 kbps.',
    Standard: 'about 160 kbps.',
    DataSaver: 'about 96 kbps, for slow or metered connections.'
  };

  var STATE_WORDS = {
    Pending: ['Waiting', 'warn'],
    Approved: ['Approved', 'accent'],
    Declined: ['Declined', ''],
    Cancelled: ['Canceled', ''],
    Done: ['In the library', 'good'],
    Failed: ['Failed', 'bad']
  };

  function qualityWord(q) {
    return q === 'Flac' ? 'FLAC' : q === 'Mp3' ? 'MP3' : 'Best quality';
  }

  function badge(text, tone) {
    return el('span', { class: 'fam-badge' + (tone ? ' ' + tone : '') }, text);
  }

  function stateBadge(state) {
    var words = STATE_WORDS[state] || [state, ''];
    return badge(words[0], words[1]);
  }

  function placeBadge(place) {
    return place === 'Away' ? badge('Away', 'warn') : badge('Home', 'good');
  }

  function meter(used, limitBytes, words) {
    var pct = limitBytes > 0 ? Math.min(100, Math.round((used / limitBytes) * 100)) : 0;
    var full = limitBytes > 0 && used >= limitBytes;
    return el('div', { class: 'fam-meter' },
      el('div', { class: 'fam-meter-line' },
        el('span', null, words),
        el('span', null, limitBytes > 0 ? F.bytes(used) + ' of ' + F.bytes(limitBytes) : F.bytes(used) + ', no limit')),
      el('div', { class: 'fam-bar' + (full ? ' full' : ''), role: 'progressbar', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': pct },
        el('span', { style: 'width:' + (limitBytes > 0 ? pct : 0) + '%' })));
  }

  function countMeter(used, limit, words) {
    var pct = limit > 0 ? Math.min(100, Math.round((used / limit) * 100)) : 0;
    return el('div', { class: 'fam-meter' },
      el('div', { class: 'fam-meter-line' },
        el('span', null, words),
        el('span', null, limit > 0 ? used + ' of ' + limit : used + ', no limit')),
      el('div', { class: 'fam-bar' + (limit > 0 && used >= limit ? ' full' : '') },
        el('span', { style: 'width:' + pct + '%' })));
  }

  // A member's abilities in plain words.
  function planLines(a) {
    var lines = [];
    var add = function (on, text) { lines.push(el('li', { class: on ? '' : 'off' }, text)); };
    if (a.addToLibrary === 'Direct') add(true, 'A heart on a song from outside the library adds it to your library.');
    else if (a.addToLibrary === 'Request') add(true, 'A heart on a song from outside the library saves it, and you can ask for a copy.');
    else add(true, 'A heart on a song from outside the library saves it to play any time.');
    if (a.addToLibrary !== 'SaveOnly') {
      add(true, 'You can ask for ' + (a.requestQuality === 'Best' ? 'the best copy there is' : a.requestQuality === 'Flac' ? 'FLAC copies' : 'MP3 copies') +
        (a.autoApprove ? ', and requests are approved at once.' : '; each request waits for approval.'));
    }
    if (a.streamCap > 0) add(true, 'Songs stream at up to ' + a.streamCap + ' kbps at home.');
    if (!a.away) add(false, 'Listening away from home is off.');
    else if (a.awayCap > 0) add(true, 'Away from home, songs stream at up to ' + a.awayCap + ' kbps.');
    else add(true, 'You can listen at home and away.');
    if (a.devicesAtOnce > 0) add(true, a.devicesAtOnce === 1 ? 'One device plays at a time.' : a.devicesAtOnce + ' devices can play at once.');
    add(a.offlineCopies, a.offlineCopies ? 'The Octo apps can keep songs for offline.' : 'Offline copies are off.');
    add(a.downloadFiles, a.downloadFiles ? 'You can download song files.' : 'Song files stream only; downloads are off.');
    add(a.share, a.share ? 'You can make share links.' : 'Share links are off.');
    add(a.familyPlaylistsEdit, a.familyPlaylistsEdit ? 'You can add to family playlists.' : 'Family playlists are to listen to.');
    if (a.cleanOnly) add(true, 'Only clean songs play.');
    return el('ul', { class: 'fam-lines' }, lines);
  }

  // ── Frame ────────────────────────────────────────────────────────────────────────────
  function navFor(s) {
    var mine = [];
    if (s.managed) {
      mine = [['me', 'Me'], ['devices', 'Devices'], ['library', 'My library'], ['saved', 'Saved'], ['requests', 'Requests'], ['playlists', 'Playlists']];
    } else {
      mine = [['me', 'Me'], ['devices', 'Devices']];
      if (s.familyEnabled && s.role !== 'Unmanaged') mine.push(['playlists', 'Playlists']);
    }
    var family = [];
    if (s.manager) family = [['family', 'Family'], ['inbox', 'Requests to approve'], ['all-devices', 'All devices'], ['load', 'Load']];
    else if (s.approver) family = [['inbox', 'Requests to approve']];
    return { mine: mine, family: family };
  }

  function defaultView(s) {
    if (!s.familyEnabled) return 'family';
    return s.manager && !s.managed ? 'family' : 'me';
  }

  function currentView() {
    var match = /^#\/([a-z-]+)/.exec(location.hash);
    return match ? match[1] : defaultView(session);
  }

  function frame(view) {
    var nav = navFor(session);
    var tabs = function (list) {
      return el('div', { class: 'fam-nav-group' }, list.map(function (item) {
        return el('a', {
          class: 'fam-tab',
          href: '#/' + item[0],
          'aria-current': item[0] === view ? 'page' : null
        }, item[1], item[0] === 'inbox' && session.pending ? el('span', { class: 'fam-tab-count' }, String(session.pending)) : null);
      }));
    };
    var main = el('main', { id: 'view' }, el('p', { class: 'fam-loading' }, 'Loading'));
    F.clear(app).appendChild(el('div', { class: 'fam-shell' },
      el('header', { class: 'fam-top' },
        el('div', { class: 'fam-brand' },
          el('img', { src: '/Assets/octo_logo.png', alt: '' }),
          el('div', null,
            el('div', { class: 'fam-brand-name' }, 'Octo Family'),
            el('div', { class: 'fam-brand-tag' }, session.displayName + ' · ' + (ROLE_WORDS[session.role] || session.role)))),
        el('div', { class: 'fam-user' },
          el('button', { class: 'fam-btn small', type: 'button', onclick: signOut }, 'Sign out'))),
      session.familyEnabled ? el('nav', { class: 'fam-nav', 'aria-label': 'Family page' }, tabs(nav.mine), nav.family.length ? tabs(nav.family) : null) : null,
      main));
    return main;
  }

  async function signOut() {
    await api('POST', '/signout', {});
    session = null;
    location.hash = '';
    renderSignIn();
  }

  function title(text, lede) {
    return [el('h1', { class: 'fam-title', tabindex: '-1' }, text), lede ? el('p', { class: 'fam-lede' }, lede) : null];
  }

  function card() {
    var node = el('section', { class: 'fam-card' });
    for (var i = 0; i < arguments.length; i++) F.append(node, arguments[i]);
    return node;
  }

  function cardHead(heading, sub, side) {
    return el('div', { class: 'fam-card-head' },
      el('div', null, el('h2', { class: 'fam-h' }, heading), sub ? el('p', { class: 'fam-sub' }, sub) : null),
      side || null);
  }

  function failed(main, reply) {
    F.clear(main).appendChild(card(el('p', { class: 'fam-error' }, F.errorOf(reply))));
  }

  // ── Sign in ──────────────────────────────────────────────────────────────────────────
  function renderSignIn(message) {
    var user = el('input', { class: 'fam-field', id: 'username', name: 'username', autocomplete: 'username', autocapitalize: 'none', spellcheck: 'false', required: true });
    var pass = el('input', { class: 'fam-field', id: 'password', name: 'password', type: 'password', autocomplete: 'current-password', required: true });
    var error = el('p', { class: 'fam-error', role: 'alert' }, message || '');
    var button = el('button', { class: 'fam-btn primary block', type: 'submit' }, 'Sign in');
    var form = el('form', { class: 'fam-form', onsubmit: async function (e) {
      e.preventDefault();
      button.disabled = true;
      error.textContent = '';
      var reply = await api('POST', '/auth', { username: user.value.trim(), password: pass.value });
      button.disabled = false;
      if (!reply.ok) {
        error.textContent = F.errorOf(reply);
        pass.value = '';
        pass.focus();
        return;
      }
      start();
    } },
    el('label', { class: 'fam-label', for: 'username' }, 'Username', user),
    el('label', { class: 'fam-label', for: 'password' }, 'Password', pass),
    error,
    button);
    F.clear(app).appendChild(el('div', { class: 'fam-center' },
      el('div', { class: 'fam-solo' },
        el('div', { class: 'fam-brand', style: 'margin-bottom:20px' },
          el('img', { src: '/Assets/octo_logo.png', alt: '' }),
          el('div', null, el('div', { class: 'fam-brand-name' }, 'Octo Family'))),
        card(
          el('h1', { class: 'fam-title' }, 'Sign in'),
          el('p', { class: 'fam-lede' }, 'Family members use the password they set from their invite link. The owner uses their Navidrome sign-in.'),
          form,
          el('p', { class: 'fam-help', style: 'margin-top:16px' }, 'Forgot your password? Ask whoever runs this server for a new sign-in link.')))));
    user.focus();
  }

  // ── Me ───────────────────────────────────────────────────────────────────────────────
  async function viewMe(main) {
    var reply = await api('GET', '/me');
    if (!reply.ok) return failed(main, reply);
    var me = reply.data;
    var a = me.abilities;
    var parts = [title('Hi, ' + me.displayName, me.managed ? 'Your plan: ' + ROLE_WORDS[me.role] + '.' : session.role === 'Owner' ? 'You own this family.' : 'Your account is not in the family, but your audio quality is yours to choose.')];

    if (me.managed) {
      var limit = a.storageLimitGb > 0 ? a.storageLimitGb * 1e9 : 0;
      parts.push(el('div', { class: 'fam-grid two' },
        card(cardHead('Your space and requests'),
          meter(me.storageUsedBytes, limit, 'Your library'),
          a.addToLibrary !== 'SaveOnly' ? countMeter(me.requestsThisWeek, a.weeklyRequestLimit, 'Requests this week') : null,
          limit > 0 && me.storageUsedBytes >= limit ? el('p', { class: 'fam-help', style: 'margin-top:12px' }, 'Your library is full, so new songs become requests.') : null),
        card(cardHead('What your plan allows'), planLines(a))));
    }
    parts.push(qualityCard(me));
    if (me.managed) {
      parts.push(el('div', { class: 'fam-grid two', style: 'margin-top:16px' }, ntfyCard(me), lastFmCard()));
      parts.push(passwordCard());
    }
    F.clear(main);
    F.append(main, parts);
  }

  function segmented(options, value, onPick, label) {
    var group = el('div', { class: 'fam-seg', role: 'group', 'aria-label': label });
    options.forEach(function (option) {
      group.appendChild(el('button', {
        type: 'button',
        'aria-pressed': option[0] === value ? 'true' : 'false',
        onclick: function () {
          Array.prototype.forEach.call(group.children, function (b) { b.setAttribute('aria-pressed', 'false'); });
          this.setAttribute('aria-pressed', 'true');
          onPick(option[0]);
        }
      }, option[1]));
    });
    return group;
  }

  function qualityCard(me) {
    var q = me.quality;
    var help = el('p', { class: 'fam-help' }, '');
    var save = async function (place, value) {
      var body = {};
      body[place] = value;
      var reply = await api('PUT', '/me/quality', body);
      if (!reply.ok) return F.toast(F.errorOf(reply), true);
      q = reply.data;
      F.toast(place === 'home' ? 'At home: ' + value : 'Away: ' + value);
      describe();
    };
    var describe = function () {
      var lines = [];
      lines.push('At home: ' + QUALITY_HELP[q.home] + ' Away: ' + QUALITY_HELP[q.away]);
      if (q.familyLimitKbps > 0) lines.push('Your family plan limits listening to ' + q.familyLimitKbps + ' kbps.');
      if (q.familyAwayLimitKbps > 0) lines.push('Your family plan limits away listening to ' + q.familyAwayLimitKbps + ' kbps.');
      lines.push('Songs from outside the library already stream at about 128 to 160 kbps, so these choices leave them as they are.');
      help.textContent = lines.join(' ');
    };
    describe();
    return card(
      cardHead('Audio quality', 'What Octo sends your devices. Each device can follow this or let its app decide, under Devices.'),
      el('div', { class: 'fam-form' },
        el('div', { class: 'fam-label' }, 'At home', segmented(QUALITIES, q.home, function (v) { save('home', v); }, 'Quality at home')),
        el('div', { class: 'fam-label' }, 'Away from home', segmented(QUALITIES, q.away, function (v) { save('away', v); }, 'Quality away from home')),
        help));
  }

  function randomTopic() {
    var letters = 'abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789';
    var bytes = new Uint8Array(24);
    crypto.getRandomValues(bytes);
    return Array.prototype.map.call(bytes, function (b) { return letters.charAt(b % letters.length); }).join('');
  }

  function ntfyCard(me) {
    var input = el('input', { class: 'fam-field fam-mono', value: me.ntfyTopic || '', placeholder: 'A private topic name', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'ntfy topic' });
    var save = async function () {
      var reply = await api('PUT', '/me/ntfy', { topic: input.value.trim() });
      if (!reply.ok) return F.toast(F.errorOf(reply), true);
      F.toast(input.value.trim() ? 'Pushes go to this topic now' : 'Pushes are off');
    };
    return card(
      cardHead('Notifications', 'Get a push when a request is approved, declined, added or fails. Install the ntfy app and subscribe to your topic.'),
      el('div', { class: 'fam-form' },
        el('div', { class: 'fam-inline' }, input),
        el('div', { class: 'fam-inline' },
          el('button', { class: 'fam-btn', type: 'button', onclick: function () { input.value = randomTopic(); } }, 'Suggest a private topic'),
          el('button', { class: 'fam-btn primary', type: 'button', onclick: save }, 'Save')),
        el('p', { class: 'fam-help' }, 'Anyone who knows a topic name can read it, so use a long random one. Leave it empty for no pushes.')));
  }

  function lastFmCard() {
    var body = el('div', { class: 'fam-form' }, el('p', { class: 'fam-sub' }, 'Loading'));
    var section = card(cardHead('Last.fm', 'Send what you play to your own Last.fm account.'), body);
    var load = async function () {
      var reply = await api('GET', '/me/lastfm');
      F.clear(body);
      if (!reply.ok) return body.appendChild(el('p', { class: 'fam-error' }, F.errorOf(reply)));
      var info = reply.data;
      if (!info.available) {
        return body.appendChild(el('p', { class: 'fam-sub' }, 'Last.fm is not set up on this server yet. Ask whoever runs it to add a Last.fm API key.'));
      }
      var user = info.user;
      if (user && user.connected) {
        body.appendChild(el('p', null, 'Connected as ', el('strong', null, user.lastFmUser || 'your account'), '.'));
        body.appendChild(el('div', null, el('button', { class: 'fam-btn', type: 'button', onclick: async function () {
          var r = await api('POST', '/me/lastfm/disconnect', {});
          if (!r.ok) return F.toast(F.errorOf(r), true);
          F.toast('Disconnected');
          load();
        } }, 'Disconnect')));
        return;
      }
      body.appendChild(el('ol', { class: 'fam-steps' },
        el('li', null, 'Press Connect and approve Octo on last.fm, signed in as you.'),
        el('li', null, 'Come back and press Finish.')));
      body.appendChild(el('div', { class: 'fam-inline' },
        el('button', { class: 'fam-btn primary', type: 'button', onclick: async function () {
          var r = await api('POST', '/me/lastfm/connect', {});
          if (!r.ok) return F.toast(F.errorOf(r), true);
          window.open(r.data.url, '_blank', 'noopener');
        } }, 'Connect'),
        el('button', { class: 'fam-btn', type: 'button', onclick: async function () {
          var r = await api('POST', '/me/lastfm/finish', {});
          if (!r.ok) return F.toast(r.status === 409 ? 'Last.fm has not seen your approval yet.' : F.errorOf(r), true);
          F.toast('Connected');
          load();
        } }, 'Finish')));
    };
    load();
    return section;
  }

  function passwordCard() {
    var current = el('input', { class: 'fam-field', type: 'password', autocomplete: 'current-password', id: 'pw-current' });
    var next = el('input', { class: 'fam-field', type: 'password', autocomplete: 'new-password', id: 'pw-next', minlength: 8 });
    var error = el('p', { class: 'fam-error', role: 'alert' });
    var form = el('form', { class: 'fam-form', onsubmit: async function (e) {
      e.preventDefault();
      error.textContent = '';
      var reply = await api('PUT', '/me/password', { current: current.value, next: next.value });
      if (!reply.ok) { error.textContent = F.errorOf(reply); return; }
      current.value = '';
      next.value = '';
      F.toast('Password changed');
    } },
    el('div', { class: 'fam-grid two' },
      el('label', { class: 'fam-label', for: 'pw-current' }, 'Current password', current),
      el('label', { class: 'fam-label', for: 'pw-next' }, 'New password', next, el('span', { class: 'fam-help' }, 'At least 8 characters.'))),
    error,
    el('div', null, el('button', { class: 'fam-btn primary', type: 'submit' }, 'Change password')));
    var section = card(cardHead('Password', 'For this page and Navidrome in a browser. Your apps use their own passwords, so they stay signed in.'), form);
    section.style.marginTop = '16px';
    return section;
  }

  // ── Devices ──────────────────────────────────────────────────────────────────────────
  function deviceKindWords(kind) {
    return { OctoApp: 'Octo app', SubsonicApp: 'App with an app password', NavidromeWeb: 'Navidrome in a browser', Detected: 'Seen by Octo' }[kind] || kind;
  }

  function deviceRow(device, options) {
    options = options || {};
    var playing = device.playing ? el('div', { class: 'fam-row-meta' }, 'Playing ', el('strong', null, device.playing.title), device.playing.artist ? ' by ' + device.playing.artist : '') : null;
    var side = el('div', { class: 'fam-row-side' });
    if (options.quality) {
      var select = el('select', { class: 'fam-field', 'aria-label': 'Quality for ' + (device.name || device.app), style: 'width:auto;min-height:32px;font-size:13px', onchange: async function () {
        var reply = await api('PUT', '/me/devices/' + encodeURIComponent(device.id), { quality: select.value });
        if (!reply.ok) return F.toast(F.errorOf(reply), true);
        F.toast(select.value === 'Account' ? 'Follows your account setting' : 'The app decides');
      } },
      el('option', { value: 'Account' }, 'Use my account setting'),
      el('option', { value: 'App' }, 'Let this app decide'));
      select.value = device.quality || 'Account';
      side.appendChild(select);
    }
    if (options.signOut && device.kind !== 'Detected') {
      side.appendChild(el('button', { class: 'fam-btn small danger', type: 'button', onclick: async function () {
        if (!confirm('Sign out ' + (device.name || device.app || 'this device') + '? It stops playing at once.')) return;
        var reply = await api('DELETE', options.signOut + encodeURIComponent(device.id));
        if (!reply.ok) return F.toast(F.errorOf(reply), true);
        F.toast('Signed out');
        if (options.after) options.after();
      } }, 'Sign out'));
    }
    return el('li', { class: 'fam-row stack' },
      el('div', { class: 'fam-row-main' },
        el('div', { class: 'fam-row-title' }, device.name || device.app || deviceKindWords(device.kind)),
        el('div', { class: 'fam-row-meta' },
          [options.owner ? device.username : null, device.app || deviceKindWords(device.kind), F.ago(device.lastSeen)].filter(Boolean).join(' · ')),
        el('div', { class: 'fam-badges' }, placeBadge(device.place), device.quality === 'App' ? badge('App decides') : null),
        playing),
      side);
  }

  async function viewDevices(main) {
    var reply = await api('GET', '/me/devices');
    if (!reply.ok) return failed(main, reply);
    var list = el('ul', { class: 'fam-list' });
    var devices = reply.data;
    if (!devices.length) list.appendChild(el('li', { class: 'fam-empty' }, session.managed ? 'No devices yet. Add one to start listening.' : 'Octo has not seen a device of yours yet.'));
    devices.forEach(function (d) {
      list.appendChild(deviceRow(d, { quality: true, signOut: session.managed ? '/me/devices/' : null, after: function () { viewDevices(main); } }));
    });
    F.clear(main);
    F.append(main, title('Devices', session.managed
      ? 'Every app you signed in, where it was last seen and what it is playing.'
      : 'The apps Octo has seen you use. Choose how each picks its audio quality.'));
    F.append(main, card(
      cardHead('Your devices', null, session.managed ? el('button', { class: 'fam-btn primary', type: 'button', onclick: addDeviceSheet }, 'Add a device') : null),
      list));
  }

  function addDeviceSheet() {
    var body = el('div', { class: 'fam-form' });
    var octo = el('button', { class: 'fam-option', type: 'button', onclick: function () { addOcto(body); } },
      el('span', { class: 'fam-option-title' }, 'The Octo app'),
      el('span', { class: 'fam-sub' }, 'Get a code and a QR code to scan.'));
    var other = el('button', { class: 'fam-option', type: 'button', onclick: function () { addOther(body); } },
      el('span', { class: 'fam-option-title' }, 'Another app'),
      el('span', { class: 'fam-sub' }, 'Symfonium, Feishin, Amperfy, Substreamer, Tempo or any Subsonic app.'));
    body.appendChild(el('div', { class: 'fam-choice' }, octo, other));
    F.openSheet('Add a device', body);
  }

  async function addOcto(body) {
    var reply = await api('POST', '/me/devices', { name: '', kind: 'OctoApp' });
    if (!reply.ok) return F.toast(F.errorOf(reply), true);
    var added = reply.data;
    var link = F.joinLink(added.server, added.username, added.pairCode);
    var expiry = el('p', { class: 'fam-help' }, F.until(added.expires));
    var timer = setInterval(function () {
      if (!document.body.contains(expiry)) return clearInterval(timer);
      expiry.textContent = F.until(added.expires);
    }, 15000);
    F.clear(body).appendChild(el('div', { class: 'fam-form' },
      el('div', { class: 'fam-qr-wrap' },
        F.qrCard(link, 'QR code to add a device'),
        el('div', { class: 'fam-form', style: 'flex:1 1 200px;min-width:0' },
          el('div', { class: 'fam-kv-label' }, 'Code'),
          el('div', { class: 'fam-code' }, added.pairCode),
          F.secret('Username', added.username),
          expiry)),
      el('ol', { class: 'fam-steps' },
        el('li', null, 'Open Octo on the new device and choose Join with a family code.'),
        el('li', null, 'Scan this QR code, or type the server address, your username and the code.')),
      F.secret('Server address', added.server),
      el('div', { class: 'fam-inline' },
        F.platform() === 'desktop' ? el('a', { class: 'fam-btn', href: F.octoLink(added.server, added.username, added.pairCode) }, 'Open in Octo on this computer') : null,
        F.copyButton(link, 'Copy the link'))));
  }

  function addOther(body) {
    var name = el('input', { class: 'fam-field', id: 'device-name', placeholder: 'Symfonium on my phone', autocomplete: 'off' });
    F.clear(body).appendChild(el('form', { class: 'fam-form', onsubmit: async function (e) {
      e.preventDefault();
      var reply = await api('POST', '/me/devices', { name: name.value.trim(), kind: 'SubsonicApp' });
      if (!reply.ok) return F.toast(F.errorOf(reply), true);
      var added = reply.data;
      F.clear(body).appendChild(F.otherApp(added.server, added.username, added.appPassword));
      viewIfShowing('devices');
    } },
    el('label', { class: 'fam-label', for: 'device-name' }, 'A name for it', name, el('span', { class: 'fam-help' }, 'So you can tell your devices apart later.')),
    el('div', null, el('button', { class: 'fam-btn primary', type: 'submit' }, 'Make an app password'))));
    name.focus();
  }

  function viewIfShowing(view) {
    if (currentView() === view) route();
  }

  // ── My library, saved, requests ─────────────────────────────────────────────────────────
  async function viewLibrary(main) {
    var offset = 0;
    var size = 100;
    var list = el('ul', { class: 'fam-list' });
    var more = el('button', { class: 'fam-btn', type: 'button', hidden: true }, 'Show more');
    var summary = el('div');
    var load = async function () {
      var reply = await api('GET', '/me/library?offset=' + offset + '&size=' + size);
      if (!reply.ok) return failed(main, reply);
      var page = reply.data;
      F.clear(summary).appendChild(meter(page.usedBytes, page.limitBytes, page.total + (page.total === 1 ? ' song' : ' songs')));
      if (!page.total) list.appendChild(el('li', { class: 'fam-empty' }, 'Nothing here yet. Songs added for you land in your library.'));
      page.songs.forEach(function (song) {
        var row = el('li', { class: 'fam-row stack' },
          el('div', { class: 'fam-row-main' },
            el('div', { class: 'fam-row-title' }, song.title),
            el('div', { class: 'fam-row-meta' }, [song.artist, song.album, F.bytes(song.size)].filter(Boolean).join(' · '))),
          el('div', { class: 'fam-row-side' }, el('button', { class: 'fam-btn small', type: 'button', onclick: async function () {
            if (!confirm('Remove ' + song.title + ' from your library?')) return;
            var r = await api('DELETE', '/me/library/' + encodeURIComponent(song.id));
            if (!r.ok) return F.toast(F.errorOf(r), true);
            row.remove();
            F.toast('Removed from your library');
          } }, 'Remove')));
        list.appendChild(row);
      });
      offset += page.songs.length;
      more.hidden = offset >= page.total;
    };
    more.addEventListener('click', load);
    F.clear(main);
    F.append(main, title('My library', 'Songs added for you. Everyone else in the family has their own; the shared library is everyone’s.'));
    F.append(main, card(summary, el('div', { style: 'margin-top:12px' }, list), el('div', { class: 'fam-actions' }, more)));
    await load();
  }

  function cover(id, words) {
    var tile = el('div', { class: 'fam-cover', 'aria-hidden': 'true' }, F.initials(words));
    if (id) {
      var img = el('img', { class: 'fam-cover', alt: '', loading: 'lazy', src: '/api/family/cover/' + encodeURIComponent(id) + '?size=96' });
      img.addEventListener('error', function () { img.replaceWith(tile); });
      return img;
    }
    return tile;
  }

  async function viewSaved(main) {
    var reply = await api('GET', '/me/saved');
    if (!reply.ok) return failed(main, reply);
    var mayAsk = session.abilities && session.abilities.addToLibrary !== 'SaveOnly';
    var songs = el('ul', { class: 'fam-list' });
    var saved = reply.data;
    if (!saved.songs.length && !saved.albums.length) songs.appendChild(el('li', { class: 'fam-empty' }, 'A heart on a song from outside the library saves it here.'));
    saved.songs.forEach(function (song) {
      var row = el('li', { class: 'fam-row stack' },
        cover(song.coverArt, song.title),
        el('div', { class: 'fam-row-main' },
          el('div', { class: 'fam-row-title' }, song.title),
          el('div', { class: 'fam-row-meta' }, [song.artist, song.album].filter(Boolean).join(' · '))),
        el('div', { class: 'fam-row-side' },
          mayAsk ? el('button', { class: 'fam-btn small primary', type: 'button', onclick: function () { askSheet(song); } }, 'Ask for a copy') : null,
          el('button', { class: 'fam-btn small', type: 'button', onclick: async function () {
            var r = await api('DELETE', '/me/saved/' + encodeURIComponent(song.id));
            if (!r.ok) return F.toast(F.errorOf(r), true);
            row.remove();
            F.toast('Removed');
          } }, 'Remove')));
      songs.appendChild(row);
    });
    saved.albums.forEach(function (album) {
      var row = el('li', { class: 'fam-row stack' },
        cover(album.coverArt, album.name),
        el('div', { class: 'fam-row-main' },
          el('div', { class: 'fam-row-title' }, album.name),
          el('div', { class: 'fam-row-meta' }, ['Album', album.artist, album.songCount ? album.songCount + ' songs' : null].filter(Boolean).join(' · '))),
        el('div', { class: 'fam-row-side' }, el('button', { class: 'fam-btn small', type: 'button', onclick: async function () {
          var r = await api('DELETE', '/me/saved/' + encodeURIComponent(album.id));
          if (!r.ok) return F.toast(F.errorOf(r), true);
          row.remove();
        } }, 'Remove')));
      songs.appendChild(row);
    });
    F.clear(main);
    F.append(main, title('Saved', 'Songs and albums you saved from outside the library. They play from where Octo found them.'));
    F.append(main, card(songs));
  }

  function askSheet(song) {
    var quality = 'Best';
    var max = (session.abilities && session.abilities.requestQuality) || 'Best';
    var rank = { Mp3: 0, Flac: 1, Best: 2 };
    var options = [['Best', 'Best there is'], ['Flac', 'FLAC'], ['Mp3', 'MP3']].filter(function (o) { return rank[o[0]] <= rank[max]; });
    quality = options[0][0];
    var body = el('div', { class: 'fam-form' },
      el('p', { class: 'fam-sub' }, song.title + (song.artist ? ' by ' + song.artist : '')),
      el('div', { class: 'fam-label' }, 'Quality', segmented(options, quality, function (v) { quality = v; }, 'Quality')),
      el('div', { class: 'fam-actions' }, el('button', { class: 'fam-btn primary', type: 'button', onclick: async function () {
        var reply = await api('POST', '/me/requests', { id: song.id, quality: quality });
        if (!reply.ok) return F.toast(F.errorOf(reply), true);
        F.closeSheet();
        F.toast(reply.data.state === 'Done' ? 'It is in your library' : reply.data.state === 'Approved' ? 'Approved' : 'Asked. You will hear back.');
      } }, 'Ask for a copy')));
    F.openSheet('Ask for a copy', body);
  }

  function requestRow(request, options) {
    options = options || {};
    var side = el('div', { class: 'fam-row-side' });
    var note = el('input', { class: 'fam-field', placeholder: 'A note for them (optional)', 'aria-label': 'Note', style: 'min-height:32px;font-size:14px;flex:1 1 180px' });
    if (options.decide && request.state === 'Pending') {
      side.appendChild(el('button', { class: 'fam-btn small primary', type: 'button', onclick: function () { options.decide(request, true, note.value); } }, 'Approve'));
      side.appendChild(el('button', { class: 'fam-btn small', type: 'button', onclick: function () { options.decide(request, false, note.value); } }, 'Decline'));
    }
    if (options.cancel && request.state === 'Pending') {
      side.appendChild(el('button', { class: 'fam-btn small', type: 'button', onclick: function () { options.cancel(request); } }, 'Cancel'));
    }
    var meta = [request.artist, request.album].filter(Boolean).join(' · ');
    var who = options.decide ? request.displayName + ' asked ' + F.ago(request.created).toLowerCase() : 'Asked ' + F.ago(request.created).toLowerCase();
    var warning = options.fullFor && options.fullFor(request) ? el('div', { class: 'fam-row-meta', style: 'color:var(--warn)' }, options.fullFor(request)) : null;
    return el('li', { class: 'fam-row stack' },
      cover(request.coverArt, request.title),
      el('div', { class: 'fam-row-main' },
        el('div', { class: 'fam-row-title' }, request.title || request.target),
        meta ? el('div', { class: 'fam-row-meta' }, meta) : null,
        el('div', { class: 'fam-row-meta' }, who),
        el('div', { class: 'fam-badges' }, stateBadge(request.state), badge(qualityWord(request.quality))),
        request.note ? el('div', { class: 'fam-row-meta' }, '“' + request.note + '”') : null,
        request.failure ? el('div', { class: 'fam-row-meta', style: 'color:var(--bad)' }, request.failure) : null,
        warning,
        options.decide && request.state === 'Pending' ? el('div', { class: 'fam-inline', style: 'margin-top:8px' }, note) : null),
      side);
  }

  async function viewRequests(main) {
    var reply = await api('GET', '/me/requests');
    if (!reply.ok) return failed(main, reply);
    var list = el('ul', { class: 'fam-list' });
    if (!reply.data.length) list.appendChild(el('li', { class: 'fam-empty' }, 'No requests yet. Ask for a copy of a saved song, or from the Octo app.'));
    reply.data.forEach(function (request) {
      list.appendChild(requestRow(request, { cancel: async function (r) {
        var answer = await api('DELETE', '/me/requests/' + encodeURIComponent(r.id));
        if (!answer.ok) return F.toast(F.errorOf(answer), true);
        F.toast('Canceled');
        viewRequests(main);
      } }));
    });
    F.clear(main);
    F.append(main, title('Requests', 'Copies you asked for, and what became of them.'));
    F.append(main, card(list));
  }

  // ── Playlists ────────────────────────────────────────────────────────────────────────
  async function viewPlaylists(main) {
    var reply = await api('GET', '/playlists');
    if (!reply.ok) return failed(main, reply);
    var list = el('ul', { class: 'fam-list' });
    if (!reply.data.length) list.appendChild(el('li', { class: 'fam-empty' }, 'No family playlists yet.'));
    reply.data.forEach(function (p) {
      var mine = p.createdBy && p.createdBy.toLowerCase() === session.username.toLowerCase();
      var side = el('div', { class: 'fam-row-side' });
      if (session.manager) {
        var approve = el('input', { type: 'checkbox', checked: p.approveAdditions, 'aria-label': 'Approve additions to ' + p.name, onchange: async function () {
          var r = await api('PUT', '/playlists/' + encodeURIComponent(p.id), { approveAdditions: approve.checked });
          if (!r.ok) { approve.checked = !approve.checked; return F.toast(F.errorOf(r), true); }
          F.toast(approve.checked ? 'Additions wait for approval' : 'Anyone can add');
        } });
        side.appendChild(el('label', { class: 'fam-switch', style: 'border:0;padding:0;align-items:center' }, el('span', { class: 'fam-small' }, 'Approve additions'), approve));
      }
      if (session.manager || mine) {
        side.appendChild(el('button', { class: 'fam-btn small', type: 'button', onclick: async function () {
          var name = prompt('New name', p.name);
          if (!name || !name.trim()) return;
          var r = await api('PUT', '/playlists/' + encodeURIComponent(p.id), { name: name.trim() });
          if (!r.ok) return F.toast(F.errorOf(r), true);
          viewPlaylists(main);
        } }, 'Rename'));
      }
      if (session.manager) {
        side.appendChild(el('button', { class: 'fam-btn small danger', type: 'button', onclick: async function () {
          if (!confirm('Delete the family playlist ' + p.name + '? Songs stay in the library.')) return;
          var r = await api('DELETE', '/playlists/' + encodeURIComponent(p.id));
          if (!r.ok) return F.toast(F.errorOf(r), true);
          viewPlaylists(main);
        } }, 'Delete'));
      }
      list.appendChild(el('li', { class: 'fam-row stack' },
        el('div', { class: 'fam-row-main' },
          el('div', { class: 'fam-row-title' }, p.name),
          el('div', { class: 'fam-row-meta' }, p.songCount + (p.songCount === 1 ? ' song' : ' songs') + ' · made by ' + p.createdBy)),
        side));
    });
    var name = el('input', { class: 'fam-field', placeholder: 'Road trip', 'aria-label': 'Playlist name' });
    var canEdit = !session.abilities || session.abilities.familyPlaylistsEdit;
    var create = canEdit ? el('form', { class: 'fam-inline', onsubmit: async function (e) {
      e.preventDefault();
      if (!name.value.trim()) return;
      var r = await api('POST', '/playlists', { name: name.value.trim() });
      if (!r.ok) return F.toast(F.errorOf(r), true);
      F.toast('Made ' + r.data.name);
      viewPlaylists(main);
    } }, name, el('button', { class: 'fam-btn primary', type: 'submit' }, 'Make a playlist')) : null;
    F.clear(main);
    F.append(main, title('Family playlists', 'Playlists everyone in the family sees and can add to from any app.'));
    F.append(main, card(create ? el('div', { style: 'margin-bottom:12px' }, create) : null, list));
  }

  // ── Family (managers) ────────────────────────────────────────────────────────────────
  async function viewFamily(main) {
    if (!session.familyEnabled) return viewFamilyOff(main);
    var replies = await Promise.all([api('GET', '/members'), api('GET', '/unrated'), api('GET', '/devices')]);
    if (!replies[0].ok) return failed(main, replies[0]);
    var members = replies[0].data;
    var devices = replies[2].ok ? replies[2].data : [];
    var parts = [title('Family', 'Everyone you share your music with. Add someone and send them their link.')];
    if (session.links === 'Copy' && session.linksMessage) {
      parts.push(el('div', { class: 'fam-banner', role: 'status' }, el('p', null, session.linksMessage)));
    }
    var unrated = replies[1].ok ? replies[1].data : null;
    if (unrated && unrated.count > 0) {
      var mark = el('button', { class: 'fam-btn', type: 'button', onclick: async function () {
        mark.disabled = true;
        var r = await api('POST', '/unrated/mark', {});
        mark.disabled = false;
        F.toast(r.ok ? r.data.message : F.errorOf(r), !r.ok);
      } }, 'Mark them now');
      parts.push(el('div', { class: 'fam-banner info' },
        el('div', { style: 'flex:1 1 auto;min-width:0' },
          el('p', null, el('strong', null, unrated.count + (unrated.count === 1 ? ' song has' : ' songs have') + ' no explicit mark.')),
          el('p', { class: 'fam-small fam-muted' }, 'Kid profiles cannot tell whether those are clean. Octo can look them up and mark them.')),
        mark));
    }
    var grid = el('div', { class: 'fam-grid two' });
    if (!members.length) {
      grid.appendChild(card(el('p', { class: 'fam-empty' }, 'No one yet. Add the first member of your family.')));
    }
    members.forEach(function (m) { grid.appendChild(memberCard(m, devices.filter(function (d) { return d.username.toLowerCase() === m.username.toLowerCase(); }))); });
    parts.push(el('div', { class: 'fam-actions', style: 'margin:0 0 16px' }, el('button', { class: 'fam-btn primary', type: 'button', onclick: addMemberSheet }, 'Add member')));
    parts.push(grid);
    F.clear(main);
    F.append(main, parts);
  }

  async function viewFamilyOff(main) {
    var parts = [title('Family is off', 'Family gives the people you share your music with their own sign-in, library and devices.')];
    if (session.role === 'Owner') {
      var report = el('div');
      var button = el('button', { class: 'fam-btn primary', type: 'button', onclick: async function () {
        button.disabled = true;
        var r = await api('POST', '/enable', { enabled: true });
        button.disabled = false;
        if (!r.ok) { F.clear(report).appendChild(el('p', { class: 'fam-error', style: 'margin-top:12px' }, F.errorOf(r))); return; }
        F.toast('Family is on');
        start();
      } }, 'Turn on Family');
      parts.push(card(
        el('p', null, 'Octo makes a folder for each member at ', el('code', null, session.membersPath), '. Navidrome needs to see that folder at the same path.'),
        el('div', { class: 'fam-actions' }, button),
        report));
    } else {
      parts.push(card(el('p', null, 'Ask whoever runs this server to turn Family on.')));
    }
    F.clear(main);
    F.append(main, parts);
  }

  function memberCard(m, devices) {
    var limit = m.storageLimitGb > 0 ? m.storageLimitGb * 1e9 : 0;
    var badges = [badge(ROLE_WORDS[m.role] || m.role, 'accent')];
    if (m.suspended) badges.push(badge('Paused', 'bad'));
    if (!m.joined) badges.push(badge('Has not joined yet', 'warn'));
    if (m.playingNow) badges.push(badge('Playing now', 'good'));
    if (m.pendingRequests) badges.push(badge(m.pendingRequests + (m.pendingRequests === 1 ? ' request waiting' : ' requests waiting'), 'warn'));
    return card(
      el('div', { class: 'fam-row', style: 'border:0;padding-top:0' },
        el('div', { class: 'fam-avatar', 'aria-hidden': 'true' }, F.initials(m.displayName)),
        el('div', { class: 'fam-row-main' },
          el('div', { class: 'fam-row-title' }, m.displayName),
          el('div', { class: 'fam-row-meta' }, m.username + ' · ' + m.devices + (m.devices === 1 ? ' device' : ' devices')),
          el('div', { class: 'fam-badges' }, badges))),
      meter(m.storageUsedBytes, limit, 'Their library'),
      el('div', { class: 'fam-actions' },
        el('button', { class: 'fam-btn small primary', type: 'button', onclick: function () { editMemberSheet(m); } }, 'Edit abilities'),
        el('button', { class: 'fam-btn small', type: 'button', onclick: function () { memberDevicesSheet(m, devices); } }, 'Devices'),
        el('button', { class: 'fam-btn small', type: 'button', onclick: function () { inviteSheet(m); } }, m.joined ? 'Send a new sign-in link' : 'Send their link again'),
        el('button', { class: 'fam-btn small', type: 'button', onclick: function () { suspend(m); } }, m.suspended ? 'Resume' : 'Pause'),
        el('button', { class: 'fam-btn small', type: 'button', onclick: function () { repair(m); } }, 'Repair'),
        el('button', { class: 'fam-btn small danger', type: 'button', onclick: function () { removeSheet(m); } }, 'Remove')));
  }

  function refreshFamily() {
    if (currentView() === 'family') route();
  }

  async function suspend(m) {
    var r = await api('PUT', '/members/' + encodeURIComponent(m.username), { suspended: !m.suspended });
    if (!r.ok) return F.toast(F.errorOf(r), true);
    F.toast(r.data.suspended ? m.displayName + ' is paused' : m.displayName + ' can listen again');
    refreshFamily();
  }

  async function repair(m) {
    var r = await api('POST', '/members/' + encodeURIComponent(m.username) + '/repair', {});
    F.toast(r.ok ? 'Repaired ' + m.displayName + '’s account' : F.errorOf(r), !r.ok);
  }

  function inviteBody(displayName, link) {
    var share = navigator.share ? el('button', { class: 'fam-btn', type: 'button', onclick: function () {
      navigator.share({ title: 'Join my Octo family', text: 'Your sign-in link for Octo:', url: link }).catch(function () {});
    } }, 'Share') : null;
    return el('div', { class: 'fam-form' },
      el('p', { class: 'fam-sub' }, 'Send ' + displayName + ' this link, or let them scan the code with their phone camera. It works once, and it is where they set their password.'),
      el('div', { class: 'fam-qr-wrap' },
        F.qrCard(link, 'QR code of the sign-in link'),
        el('div', { class: 'fam-form', style: 'flex:1 1 220px;min-width:0' },
          F.secret('Sign-in link', link),
          share ? el('div', { class: 'fam-inline' }, share) : null)));
  }

  async function inviteSheet(m) {
    var r = await api('POST', '/members/' + encodeURIComponent(m.username) + '/invite', {});
    if (!r.ok) return F.toast(F.errorOf(r), true);
    F.openSheet('A new sign-in link for ' + m.displayName, inviteBody(m.displayName, r.data.inviteLink));
  }

  function memberDevicesSheet(m, devices) {
    var list = el('ul', { class: 'fam-list' });
    if (!devices.length) list.appendChild(el('li', { class: 'fam-empty' }, 'No devices yet.'));
    devices.forEach(function (d) {
      list.appendChild(deviceRow(d, { signOut: '/devices/', after: function () { F.closeSheet(); refreshFamily(); } }));
    });
    F.openSheet(m.displayName + '’s devices', list);
  }

  async function librariesPicker(current) {
    var reply = await api('GET', '/libraries');
    var shared = reply.ok ? reply.data.filter(function (l) { return !l.personal; }) : [];
    var all = current === 'All' || current === undefined;
    var chosen = Array.isArray(current) ? current.slice() : [];
    var box = el('div', { class: 'fam-form' });
    var allSwitch = el('input', { type: 'checkbox', checked: all, 'aria-label': 'Every shared library' });
    var ids = el('div', { class: 'fam-form', hidden: all });
    shared.forEach(function (l) {
      var check = el('input', { type: 'checkbox', checked: chosen.indexOf(l.id) >= 0 || all, 'data-id': l.id });
      ids.appendChild(el('label', { class: 'fam-switch' }, el('span', { class: 'fam-switch-text' }, el('span', { class: 'fam-switch-title' }, l.name), el('div', { class: 'fam-help' }, l.path)), check));
    });
    allSwitch.addEventListener('change', function () { ids.hidden = allSwitch.checked; });
    box.appendChild(el('label', { class: 'fam-switch' },
      el('span', { class: 'fam-switch-text' }, el('span', { class: 'fam-switch-title' }, 'Every shared library'), el('div', { class: 'fam-help' }, 'Libraries you add later are shared too.')),
      allSwitch));
    box.appendChild(ids);
    box.value = function () {
      if (allSwitch.checked) return 'All';
      return Array.prototype.filter.call(ids.querySelectorAll('input'), function (i) { return i.checked; })
        .map(function (i) { return Number(i.getAttribute('data-id')); });
    };
    return box;
  }

  async function addMemberSheet() {
    var preset = 'Listener';
    var username = el('input', { class: 'fam-field', id: 'new-username', autocapitalize: 'none', spellcheck: 'false', autocomplete: 'off', placeholder: 'alex', required: true });
    var displayName = el('input', { class: 'fam-field', id: 'new-name', autocomplete: 'off', placeholder: 'Alex' });
    var presetHelp = el('p', { class: 'fam-help' }, PRESETS[preset]);
    var presets = [['Listener', 'Listener'], ['Member', 'Member'], ['Kid', 'Kid']];
    if (session.role === 'Owner') presets.push(['CoAdmin', 'Co-admin']);
    var error = el('p', { class: 'fam-error', role: 'alert' });
    var libraries = await librariesPicker('All');
    var adopt = false;
    var submit = el('button', { class: 'fam-btn primary', type: 'submit' }, 'Add');
    var form = el('form', { class: 'fam-form', onsubmit: async function (e) {
      e.preventDefault();
      error.textContent = '';
      submit.disabled = true;
      var r = await api('POST', '/members', {
        username: username.value.trim().toLowerCase(),
        displayName: displayName.value.trim(),
        preset: preset,
        sharedLibraries: libraries.value(),
        adopt: adopt
      });
      submit.disabled = false;
      if (!r.ok) {
        error.textContent = F.errorOf(r);
        if (r.status === 409 && /already has a Navidrome account/.test(F.errorOf(r))) {
          adopt = true;
          submit.textContent = 'Bring them into the family';
        }
        return;
      }
      var member = r.data.member;
      F.openSheet(member.displayName + ' is in the family', inviteBody(member.displayName, r.data.inviteLink));
      refreshFamily();
    } },
    el('label', { class: 'fam-label', for: 'new-username' }, 'Username', username, el('span', { class: 'fam-help' }, 'Lowercase letters, numbers, dots, dashes or underscores.')),
    el('label', { class: 'fam-label', for: 'new-name' }, 'Name', displayName),
    el('div', { class: 'fam-label' }, 'What they can do', segmented(presets, preset, function (v) { preset = v; presetHelp.textContent = PRESETS[v]; }, 'Preset'), presetHelp),
    el('div', { class: 'fam-label' }, 'Shared music they see', libraries),
    error,
    el('div', { class: 'fam-actions' }, submit));
    F.openSheet('Add member', form);
    username.focus();
  }

  async function editMemberSheet(m) {
    var preset = m.preset;
    var values = Object.assign({}, m.abilities);
    var display = el('input', { class: 'fam-field', id: 'edit-name', value: m.displayName });
    var fields = el('div');
    var libraries = await librariesPicker(m.sharedLibraries);
    var presetHelp = el('p', { class: 'fam-help' }, PRESETS[preset]);
    var drawFields = function () {
      F.clear(fields);
      ABILITY_FIELDS.forEach(function (f) {
        if (f.owner && session.role !== 'Owner') return;
        var control;
        if (f.kind === 'switch') {
          control = el('input', { type: 'checkbox', checked: values[f.key], onchange: function () { values[f.key] = control.checked; } });
        } else if (f.kind === 'number') {
          control = el('input', { class: 'fam-field', type: 'number', min: 0, step: 1, value: values[f.key], inputmode: 'numeric', oninput: function () { values[f.key] = Math.max(0, parseInt(control.value, 10) || 0); } });
        } else {
          control = el('select', { class: 'fam-field', onchange: function () { values[f.key] = control.value; } },
            f.options.map(function (o) { return el('option', { value: o[0] }, o[1]); }));
          control.value = values[f.key];
        }
        control.setAttribute('aria-label', f.title);
        fields.appendChild(el('label', { class: 'fam-switch' },
          el('span', { class: 'fam-switch-text' }, el('span', { class: 'fam-switch-title' }, f.title), f.help ? el('div', { class: 'fam-help' }, f.help) : null),
          control));
      });
    };
    drawFields();
    var presets = [['Listener', 'Listener'], ['Member', 'Member'], ['Kid', 'Kid']];
    if (session.role === 'Owner' || m.preset === 'CoAdmin') presets.push(['CoAdmin', 'Co-admin']);
    var error = el('p', { class: 'fam-error', role: 'alert' });
    var form = el('form', { class: 'fam-form', onsubmit: async function (e) {
      e.preventDefault();
      var base = PRESET_ABILITIES[preset] || {};
      var edits = {};
      ABILITY_FIELDS.forEach(function (f) {
        if (values[f.key] !== base[f.key]) edits[f.key] = values[f.key];
      });
      var r = await api('PUT', '/members/' + encodeURIComponent(m.username), {
        displayName: display.value.trim(),
        preset: preset,
        edits: edits,
        sharedLibraries: libraries.value()
      });
      if (!r.ok) { error.textContent = F.errorOf(r); return; }
      F.closeSheet();
      F.toast('Saved');
      refreshFamily();
    } },
    el('label', { class: 'fam-label', for: 'edit-name' }, 'Name', display),
    el('div', { class: 'fam-label' }, 'Start from', segmented(presets, preset, function (v) {
      preset = v;
      presetHelp.textContent = PRESETS[v];
      values = Object.assign({}, PRESET_ABILITIES[v]);
      drawFields();
    }, 'Preset'), presetHelp),
    el('div', { class: 'fam-label' }, 'Abilities', fields),
    el('div', { class: 'fam-label' }, 'Shared music they see', libraries),
    el('p', { class: 'fam-help' }, 'Outside songs stream at about 128 to 160 kbps whatever the limits say.'),
    error,
    el('div', { class: 'fam-actions' }, el('button', { class: 'fam-btn primary', type: 'submit' }, 'Save')));
    F.openSheet('Edit ' + m.displayName, form);
  }

  function removeSheet(m) {
    var files = 'keep';
    var confirmInput = el('input', { class: 'fam-field', id: 'confirm-name', autocapitalize: 'none', spellcheck: 'false', autocomplete: 'off' });
    var error = el('p', { class: 'fam-error', role: 'alert' });
    var button = el('button', { class: 'fam-btn danger', type: 'submit', disabled: true }, 'Remove ' + m.displayName);
    confirmInput.addEventListener('input', function () { button.disabled = confirmInput.value.trim().toLowerCase() !== m.username.toLowerCase(); });
    var choices = [
      ['keep', 'Keep them aside', 'Songs only they had are kept in a folder for you.'],
      ['promote', 'Add them to the shared library', 'Everyone can play them.'],
      ['delete', 'Delete them', 'Songs only they had go to Octo’s trash.']
    ];
    var help = el('p', { class: 'fam-help' }, choices[0][2]);
    var form = el('form', { class: 'fam-form', onsubmit: async function (e) {
      e.preventDefault();
      button.disabled = true;
      var r = await api('DELETE', '/members/' + encodeURIComponent(m.username), { confirm: confirmInput.value.trim(), files: files });
      button.disabled = false;
      if (!r.ok) { error.textContent = F.errorOf(r); return; }
      F.closeSheet();
      F.toast(m.displayName + ' was removed');
      refreshFamily();
    } },
    el('p', { class: 'fam-sub' }, 'Their account, devices and own library are removed. Shared music is not touched.'),
    el('div', { class: 'fam-label' }, 'Their songs', segmented(choices.map(function (c) { return [c[0], c[1]]; }), files, function (v) {
      files = v;
      help.textContent = choices.filter(function (c) { return c[0] === v; })[0][2];
    }, 'Their songs'), help),
    el('label', { class: 'fam-label', for: 'confirm-name' }, 'Type ' + m.username + ' to confirm', confirmInput),
    error,
    el('div', { class: 'fam-actions' }, button));
    F.openSheet('Remove ' + m.displayName, form);
  }

  // ── Inbox, all devices, load ─────────────────────────────────────────────────────────
  async function viewInbox(main) {
    var filter = (/[?&]state=([A-Za-z]+)/.exec(location.hash) || [])[1] || 'Pending';
    var replies = await Promise.all([
      api('GET', '/requests' + (filter === 'All' ? '' : '?state=' + filter)),
      session.manager ? api('GET', '/members') : Promise.resolve({ ok: false })
    ]);
    if (!replies[0].ok) return failed(main, replies[0]);
    var members = replies[1].ok ? replies[1].data : [];
    var fullFor = function (request) {
      var m = members.filter(function (x) { return x.username === request.username; })[0];
      if (!m || !m.storageLimitGb) return null;
      var limit = m.storageLimitGb * 1e9;
      if (m.storageUsedBytes < limit) return null;
      return 'Their library is full (' + F.bytes(m.storageUsedBytes) + ' of ' + m.storageLimitGb + ' GB). Approving adds more anyway.';
    };
    var list = el('ul', { class: 'fam-list' });
    if (!replies[0].data.length) list.appendChild(el('li', { class: 'fam-empty' }, filter === 'Pending' ? 'Nothing is waiting for you.' : 'Nothing here.'));
    replies[0].data.forEach(function (request) {
      list.appendChild(requestRow(request, { fullFor: fullFor, decide: async function (r, approve, note) {
        var answer = await api('POST', '/requests/' + encodeURIComponent(r.id) + (approve ? '/approve' : '/decline'), { note: note });
        if (!answer.ok) return F.toast(F.errorOf(answer), true);
        F.toast(approve ? 'Approved' : 'Declined');
        session.pending = Math.max(0, (session.pending || 1) - 1);
        route();
      } }));
    });
    var filters = segmented([['Pending', 'Waiting'], ['Approved', 'Approved'], ['Done', 'Done'], ['Declined', 'Declined'], ['Failed', 'Failed'], ['All', 'All']], filter, function (v) {
      location.hash = '#/inbox?state=' + v;
    }, 'Show');
    F.clear(main);
    F.append(main, title('Requests to approve', 'Copies family members asked for. They hear what you decide.'));
    F.append(main, card(el('div', { style: 'margin-bottom:12px' }, filters), list));
  }

  async function viewAllDevices(main) {
    var reply = await api('GET', '/devices');
    if (!reply.ok) return failed(main, reply);
    var list = el('ul', { class: 'fam-list' });
    if (!reply.data.length) list.appendChild(el('li', { class: 'fam-empty' }, 'No devices yet.'));
    reply.data.forEach(function (d) {
      list.appendChild(deviceRow(d, { owner: true, signOut: '/devices/', after: function () { viewAllDevices(main); } }));
    });
    F.clear(main);
    F.append(main, title('All devices', 'Every family member’s devices, and the apps Octo has seen for everyone else.'));
    F.append(main, card(list));
  }

  async function viewLoad(main) {
    var reply = await api('GET', '/load');
    if (!reply.ok) return failed(main, reply);
    var load = reply.data;
    var streams = el('ul', { class: 'fam-list' });
    if (!load.streams.length) streams.appendChild(el('li', { class: 'fam-empty' }, 'Nothing is playing right now.'));
    load.streams.forEach(function (s) {
      streams.appendChild(el('li', { class: 'fam-row' },
        el('div', { class: 'fam-row-main' },
          el('div', { class: 'fam-row-title' }, s.songTitle || 'A song'),
          el('div', { class: 'fam-row-meta' }, s.username + ' · ' + s.device)),
        el('div', { class: 'fam-row-side' }, placeBadge(s.place), badge(s.kbps ? s.kbps + ' kbps' : 'Original'))));
    });
    var queue = el('ul', { class: 'fam-list' });
    if (!load.queue.length) queue.appendChild(el('li', { class: 'fam-empty' }, 'No downloads waiting.'));
    load.queue.forEach(function (q) {
      queue.appendChild(el('li', { class: 'fam-row' }, el('div', { class: 'fam-row-main' }, el('div', { class: 'fam-row-title' }, q.username)), el('div', { class: 'fam-row-side' }, badge(q.waiting + ' waiting'))));
    });
    F.clear(main);
    F.append(main, title('Load', 'How busy the server is right now.'));
    F.append(main, card(el('div', { class: 'fam-stats' },
      el('div', { class: 'fam-stat' }, el('span', { class: 'fam-stat-n' }, String(load.streams.length)), el('span', { class: 'fam-sub' }, 'Streams playing')),
      el('div', { class: 'fam-stat' }, el('span', { class: 'fam-stat-n' }, load.shim.limit ? load.shim.active + ' of ' + load.shim.limit : String(load.shim.active)), el('span', { class: 'fam-sub' }, 'Outside songs streaming')),
      el('div', { class: 'fam-stat' }, el('span', { class: 'fam-stat-n' }, String(load.shim.ytdlpRunning)), el('span', { class: 'fam-sub' }, 'Lookups running')),
      el('div', { class: 'fam-stat' }, el('span', { class: 'fam-stat-n' }, load.uploadKbps + ' kbps'), el('span', { class: 'fam-sub' }, 'Sent out now')))));
    F.append(main, el('div', { class: 'fam-grid two', style: 'margin-top:16px' },
      card(cardHead('Playing now'), streams),
      card(cardHead('Downloads waiting'), queue)));
  }

  // ── Routing ──────────────────────────────────────────────────────────────────────────
  var VIEWS = {
    me: viewMe,
    devices: viewDevices,
    library: viewLibrary,
    saved: viewSaved,
    requests: viewRequests,
    playlists: viewPlaylists,
    family: viewFamily,
    inbox: viewInbox,
    'all-devices': viewAllDevices,
    load: viewLoad
  };

  function allowed(view) {
    var nav = navFor(session);
    return nav.mine.concat(nav.family).some(function (item) { return item[0] === view; }) || (view === 'family' && !session.familyEnabled);
  }

  async function route() {
    if (!session) return;
    var view = currentView();
    if (!VIEWS[view] || !allowed(view)) view = defaultView(session);
    var main = frame(view);
    document.title = 'Octo Family';
    await VIEWS[view](main);
    var heading = main.querySelector('h1');
    if (heading && document.activeElement === document.body) heading.focus({ preventScroll: true });
  }

  async function start() {
    var reply = await api('GET', '/session');
    if (reply.status === 401) return renderSignIn();
    if (!reply.ok) return renderSignIn(F.errorOf(reply));
    session = reply.data;
    session.pending = 0;
    if (session.familyEnabled) {
      var me = await api('GET', '/me');
      if (me.ok) session.abilities = me.data.abilities;
      if (session.approver) {
        var waiting = await api('GET', '/requests?state=Pending');
        if (waiting.ok) session.pending = waiting.data.length;
      }
    }
    route();
  }

  // An invite link opened here goes to the join page.
  if (/invite=/.test(location.hash) || /[#&]c=\d/.test(location.hash)) {
    location.replace('/family/join' + location.hash);
    return;
  }
  window.addEventListener('hashchange', function () { F.closeSheet(); route(); });
  start();
})();
