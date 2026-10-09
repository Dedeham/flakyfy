/*
 * The Family page, on the dashboard's own shell: a glass sidebar of sections, a page header
 * with its icon tile, and glass cards. Everyone sees "You" (overview, devices, requests, saved
 * songs, their library, audio quality and account); people who manage the family also see
 * "Family" (members, requests to approve, all devices, family playlists and activity).
 * Everything goes through /api/family; the page keeps no secrets of its own.
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

  // The abilities a manager edits, grouped as the edit sheet shows them, each with one line on
  // what it does.
  var ABILITY_GROUPS = [
    { title: 'Listening', fields: [
      { key: 'away', title: 'Listen away from home', help: 'Off keeps their apps and links to your home network.', kind: 'switch' },
      { key: 'streamCap', title: 'Highest quality at home, in kbps', help: '0 means the original file. Their own choice can only go lower.', kind: 'number' },
      { key: 'awayCap', title: 'Highest quality away from home, in kbps', help: '0 means the same as at home.', kind: 'number' },
      { key: 'devicesAtOnce', title: 'Devices playing at once', help: '0 means no limit. The newest device plays.', kind: 'number' },
      { key: 'offlineCopies', title: 'Keep songs for offline', help: 'In the Octo apps, for planes and bad signal.', kind: 'switch' },
      { key: 'downloadFiles', title: 'Download song files', help: 'Off means songs stream only.', kind: 'switch' },
      { key: 'cleanOnly', title: 'Clean songs only', help: 'Songs marked explicit do not play for them.', kind: 'switch' }
    ] },
    { title: 'Adding music', fields: [
      { key: 'addToLibrary', title: 'A heart on a song from outside the library', help: 'What happens when they heart a song Octo found elsewhere.', kind: 'choice',
        options: [['Direct', 'Add it to their library'], ['Request', 'Save it; they can ask for a copy'], ['SaveOnly', 'Save it only']] },
      { key: 'requestQuality', title: 'Best copy they can ask for', help: 'The highest quality their requests fetch.', kind: 'choice',
        options: [['Best', 'Best there is'], ['Flac', 'FLAC'], ['Mp3', 'MP3']] },
      { key: 'autoApprove', title: 'Approve their requests at once', help: 'Off means each request waits for you.', kind: 'switch' },
      { key: 'instantFromFamily', title: 'Add songs the family already has at once', help: 'No download and no extra space on disk.', kind: 'switch' },
      { key: 'importPlaylists', title: 'Import playlists', help: 'From Spotify and other services.', kind: 'switch' }
    ] },
    { title: 'Sharing', fields: [
      { key: 'share', title: 'Make share links', help: 'Links people outside the family can open.', kind: 'switch' },
      { key: 'familyPlaylistsEdit', title: 'Add to family playlists', help: 'Off means family playlists are to listen to.', kind: 'switch' }
    ] },
    { title: 'Limits', fields: [
      { key: 'weeklyRequestLimit', title: 'Requests a week', help: '0 means no limit.', kind: 'number' },
      { key: 'storageLimitGb', title: 'Space for their own library, in GB', help: '0 means no limit. When it is full, adding a song becomes a request.', kind: 'number' }
    ] },
    { title: 'Managing', owner: true, fields: [
      { key: 'manageFamily', title: 'Manage the family', help: 'Add and edit members, and see everyone’s devices.', kind: 'switch' },
      { key: 'approveRequests', title: 'Approve requests', help: 'Decide on everyone’s requests.', kind: 'switch' }
    ] }
  ];
  var ABILITY_FIELDS = ABILITY_GROUPS.reduce(function (all, g) { return all.concat(g.fields); }, []);

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
        el('span', { class: 'fam-meter-n' }, limitBytes > 0 ? F.bytes(used) + ' of ' + F.bytes(limitBytes) : F.bytes(used) + ', no limit')),
      el('div', { class: 'fam-bar' + (full ? ' full' : ''), role: 'progressbar', 'aria-label': words, 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': pct },
        el('span', { style: 'width:' + (limitBytes > 0 ? pct : 0) + '%' })));
  }

  function countMeter(used, limit, words) {
    var pct = limit > 0 ? Math.min(100, Math.round((used / limit) * 100)) : 0;
    return el('div', { class: 'fam-meter' },
      el('div', { class: 'fam-meter-line' },
        el('span', null, words),
        el('span', { class: 'fam-meter-n' }, limit > 0 ? used + ' of ' + limit : used + ', no limit')),
      el('div', { class: 'fam-bar' + (limit > 0 && used >= limit ? ' full' : ''), role: 'progressbar', 'aria-label': words, 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': pct },
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

  // ── Building blocks on the dashboard's classes ───────────────────────────────────────
  function pageHeader(icon, title, desc, actions) {
    return el('header', { class: 'page-header' },
      el('span', { class: 'page-icon', 'aria-hidden': 'true' }, F.icon(icon)),
      el('div', null, el('h1', { tabindex: '-1' }, title), desc ? el('p', null, desc) : null),
      actions ? el('div', { class: 'fam-head-actions' }, actions) : null);
  }

  // A glass card with an optional title, a line under it and something on the right.
  function card(title, desc, side) {
    var node = el('section', { class: 'info-card fam-card' });
    if (title || side) {
      node.appendChild(el('div', { class: 'fam-card-head' },
        el('div', null, title ? el('h2', { class: 'info-card-title' }, title) : null, desc ? el('p', { class: 'fam-card-desc' }, desc) : null),
        side || null));
    }
    for (var i = 3; i < arguments.length; i++) F.append(node, arguments[i]);
    return node;
  }

  // A glass card of rows, as the dashboard's settings cards are.
  function rowsCard(rows) {
    var node = el('div', { class: 'set-card fam-rows' });
    F.append(node, rows);
    return node;
  }

  // An empty list: what would be here, and the thing to do about it.
  function empty(icon, text, action) {
    return el('div', { class: 'fam-empty' },
      el('span', { class: 'fam-empty-ico', 'aria-hidden': 'true' }, F.icon(icon)),
      el('p', null, text),
      action || null);
  }

  function switchControl(checked, label, onchange) {
    var input = el('input', { type: 'checkbox', checked: checked, 'aria-label': label, onchange: onchange });
    return { input: input, node: el('label', { class: 'switch' }, input, el('span', { class: 'sw-track' }), el('span', { class: 'sw-thumb' })) };
  }

  function segmented(options, value, onPick, label) {
    var group = el('div', { class: 'seg fam-seg', role: 'group', 'aria-label': label });
    options.forEach(function (option) {
      group.appendChild(el('button', {
        type: 'button',
        class: option[0] === value ? 'active' : '',
        'aria-pressed': option[0] === value ? 'true' : 'false',
        onclick: function () {
          Array.prototype.forEach.call(group.children, function (b) { b.setAttribute('aria-pressed', 'false'); b.classList.remove('active'); });
          this.setAttribute('aria-pressed', 'true');
          this.classList.add('active');
          onPick(option[0]);
        }
      }, option[1]));
    });
    return group;
  }

  function setRow(title, desc, control, stack) {
    return el('div', { class: 'set-row' + (stack ? ' stack' : '') },
      el('div', { class: 'set-info' }, el('span', { class: 'set-info-t' }, title), desc ? el('span', { class: 'set-info-d' }, desc) : null),
      control ? el('div', { class: 'set-ctrl' }, control) : null);
  }

  function failed(main, reply) {
    F.clear(main).appendChild(el('p', { class: 'notice notice-error', role: 'alert' }, F.errorOf(reply)));
  }

  function confirmIn(words) {
    return window.confirm(words);
  }

  // ── Navigation ───────────────────────────────────────────────────────────────────────
  var VIEWS_INFO = {
    overview: ['Overview', 'pulse'],
    devices: ['Devices', 'phone'],
    requests: ['Requests', 'tray'],
    saved: ['Saved', 'heart'],
    library: ['My library', 'folder'],
    quality: ['Audio quality', 'sliders'],
    account: ['Account', 'key'],
    members: ['Members', 'users'],
    inbox: ['Requests to approve', 'checkCircle'],
    'all-devices': ['All devices', 'plugs'],
    playlists: ['Family playlists', 'playlist'],
    activity: ['Activity', 'drives']
  };

  function navFor(s) {
    var you = s.managed ? ['overview', 'devices', 'requests', 'saved', 'library', 'quality', 'account'] : ['devices', 'quality'];
    var family = [];
    if (s.familyEnabled) {
      if (s.manager) family = ['members', 'inbox', 'all-devices', 'playlists', 'activity'];
      else if (s.approver) family = ['inbox', 'playlists'];
      else if (s.role !== 'Unmanaged') family = ['playlists'];
    } else if (s.owner) {
      family = ['members'];
    }
    return { you: you, family: family };
  }

  function defaultView(s) {
    if (!s.familyEnabled) return s.owner ? 'members' : 'devices';
    return s.managed ? 'overview' : s.manager ? 'members' : 'devices';
  }

  // Old addresses from before the sidebar still land somewhere sensible.
  var OLD_VIEWS = { me: 'overview', family: 'members', load: 'activity' };

  function currentView() {
    var match = /^#\/([a-z-]+)/.exec(location.hash);
    var view = match ? match[1] : defaultView(session);
    return OLD_VIEWS[view] || view;
  }

  function navItem(view, current) {
    var info = VIEWS_INFO[view];
    var count = view === 'inbox' && session.pending ? el('span', { class: 'sidebar-nav-badge bad' }, String(session.pending)) : null;
    return el('a', {
      class: 'sidebar-nav-item' + (view === current ? ' active' : ''),
      href: '#/' + view,
      'aria-current': view === current ? 'page' : null,
      onclick: closeDrawer
    }, F.icon(info[1]), el('span', null, info[0]), count, count ? el('span', { class: 'visually-hidden' }, ' waiting') : null);
  }

  function openDrawer() {
    document.body.classList.add('fam-drawer-open');
    var current = document.querySelector('.sidebar .sidebar-nav-item.active') || document.querySelector('.sidebar .sidebar-nav-item');
    if (current) current.focus();
  }

  function closeDrawer() {
    document.body.classList.remove('fam-drawer-open');
  }

  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape' && document.body.classList.contains('fam-drawer-open')) {
      closeDrawer();
      var menu = document.getElementById('fam-menu');
      if (menu) menu.focus();
    }
  });

  function frame(view) {
    var nav = navFor(session);
    var sections = [el('div', { class: 'sidebar-nav-section' },
      el('div', { class: 'sidebar-nav-label' }, 'You'),
      nav.you.map(function (v) { return navItem(v, view); }))];
    if (nav.family.length) {
      sections.push(el('div', { class: 'sidebar-nav-section' },
        el('div', { class: 'sidebar-nav-label' }, 'Family'),
        nav.family.map(function (v) { return navItem(v, view); })));
    }
    if (session.owner) {
      sections.push(el('div', { class: 'sidebar-nav-section' },
        el('div', { class: 'sidebar-nav-label' }, 'Server'),
        el('a', { class: 'sidebar-nav-item', href: '/admin/' }, F.icon('out'), el('span', null, 'Server dashboard'))));
    }
    var main = el('main', { class: 'content', id: 'view' }, el('p', { class: 'state' }, 'Loading'));
    var sidebar = el('aside', { class: 'sidebar', id: 'fam-sidebar', 'aria-label': 'Family page' },
      el('div', { class: 'sidebar-brand' },
        el('img', { src: '/Assets/octo_logo.png', alt: '', class: 'sidebar-logo' }),
        el('div', { class: 'sidebar-brand-text' },
          el('div', { class: 'sidebar-brand-name' }, 'Octo Family'),
          el('div', { class: 'sidebar-brand-tag' }, session.displayName + ' · ' + (ROLE_WORDS[session.role] || session.role)))),
      el('nav', { class: 'sidebar-nav', 'aria-label': 'Sections' }, sections),
      el('div', { class: 'sidebar-footer' },
        el('button', { class: 'btn btn-ghost btn-block', type: 'button', onclick: signOut }, F.icon('signin'), el('span', null, 'Sign out'))));
    var top = el('header', { class: 'fam-topbar' },
      el('button', { class: 'btn btn-ghost btn-icon', id: 'fam-menu', type: 'button', 'aria-label': 'Open the menu', 'aria-controls': 'fam-sidebar', onclick: openDrawer }, F.icon('menu')),
      el('img', { src: '/Assets/octo_logo.png', alt: '', class: 'fam-topbar-logo' }),
      el('span', { class: 'fam-topbar-name' }, 'Octo Family'));
    F.clear(app).appendChild(el('div', { class: 'app fam-app' },
      top,
      el('div', { class: 'fam-scrim', onclick: closeDrawer, 'aria-hidden': 'true' }),
      sidebar,
      main));
    return main;
  }

  async function signOut() {
    await api('POST', '/signout', {});
    session = null;
    location.hash = '';
    renderSignIn();
  }

  // ── Sign in ──────────────────────────────────────────────────────────────────────────
  function renderSignIn(message) {
    var user = el('input', { class: 'set-input', id: 'username', name: 'username', autocomplete: 'username', autocapitalize: 'none', spellcheck: 'false', required: true });
    var pass = el('input', { class: 'set-input', id: 'password', name: 'password', type: 'password', autocomplete: 'current-password', required: true });
    var error = el('p', { class: 'field-error', role: 'alert' }, message || '');
    var button = el('button', { class: 'btn btn-primary btn-block fam-big', type: 'submit' }, 'Sign in');
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
        el('div', { class: 'sidebar-brand fam-solo-brand' },
          el('img', { src: '/Assets/octo_logo.png', alt: '', class: 'sidebar-logo' }),
          el('div', { class: 'sidebar-brand-text' }, el('div', { class: 'sidebar-brand-name' }, 'Octo Family'), el('div', { class: 'sidebar-brand-tag' }, location.host))),
        el('section', { class: 'info-card fam-card' },
          el('h1', { class: 'fam-solo-title' }, 'Sign in'),
          el('p', { class: 'fam-card-desc' }, 'Family members use the password they set from their invite link. The owner uses their Navidrome sign-in.'),
          form,
          el('p', { class: 'fam-help fam-gap' }, 'Forgot your password? Ask whoever runs this server for a new sign-in link.')))));
    user.focus();
  }

  // ── You: Overview ────────────────────────────────────────────────────────────────────
  function nextStep(icon, title, desc, action) {
    return el('section', { class: 'callout fam-next' },
      el('span', { class: 'page-icon fam-next-ico', 'aria-hidden': 'true' }, F.icon(icon)),
      el('div', { class: 'fam-next-text' }, el('div', { class: 'callout-title' }, title), el('p', null, desc)),
      el('div', { class: 'fam-next-act' }, action));
  }

  async function viewOverview(main) {
    var replies = await Promise.all([api('GET', '/me'), api('GET', '/me/devices'), api('GET', '/me/requests')]);
    if (!replies[0].ok) return failed(main, replies[0]);
    var me = replies[0].data;
    var a = me.abilities;
    var devices = replies[1].ok ? replies[1].data : [];
    var requests = replies[2].ok ? replies[2].data : [];
    var limit = a.storageLimitGb > 0 ? a.storageLimitGb * 1e9 : 0;
    var playing = devices.filter(function (d) { return d.playing; });
    var addButton = function (words) {
      return el('button', { class: 'btn btn-primary', type: 'button', onclick: function () { deviceDialog(null); } }, F.icon('plus'), words);
    };

    // One next step, the most useful first.
    var ready = requests.filter(function (r) { return r.state === 'Done'; })[0];
    var waiting = requests.filter(function (r) { return r.state === 'Pending'; });
    var next;
    if (!devices.length) {
      next = nextStep('phone', 'Add your first device', 'Scan a code with the Octo app, or sign in any Subsonic app with an app password.', addButton('Add a device'));
    } else if (ready) {
      next = nextStep('checkCircle', 'Your request for ' + (ready.title || 'a song') + ' is ready', 'It is in your library now, on every device.',
        el('a', { class: 'btn', href: '#/requests' }, 'See your requests'));
    } else if (waiting.length) {
      next = nextStep('clock', waiting.length === 1 ? 'One request is waiting for approval' : waiting.length + ' requests are waiting for approval', 'You hear back when they are decided.',
        el('a', { class: 'btn', href: '#/requests' }, 'See your requests'));
    } else if (limit > 0 && me.storageUsedBytes >= limit) {
      next = nextStep('warning', 'Your library is full', 'New songs become requests until there is room. Remove songs you no longer play.',
        el('a', { class: 'btn', href: '#/library' }, 'Open my library'));
    } else {
      next = nextStep('heart', 'You are all set', 'Heart songs in your app, and they show up here.',
        el('a', { class: 'btn', href: '#/saved' }, 'Open saved songs'));
    }

    var counts = {};
    requests.forEach(function (r) { counts[r.state] = (counts[r.state] || 0) + 1; });
    var statusBadges = Object.keys(counts).map(function (state) {
      var words = STATE_WORDS[state] || [state, ''];
      return badge(counts[state] + ' ' + words[0].toLowerCase(), words[1]);
    });

    var nowPlaying = playing.length
      ? el('ul', { class: 'fam-list' }, playing.map(function (d) {
        return el('li', { class: 'fam-row' },
          el('span', { class: 'fam-row-ico', 'aria-hidden': 'true' }, F.icon('radio')),
          el('div', { class: 'fam-row-main' },
            el('div', { class: 'fam-row-title' }, d.playing.title),
            el('div', { class: 'fam-row-meta' }, [d.playing.artist, d.name || d.app].filter(Boolean).join(' · '))),
          placeBadge(d.place));
      }))
      : el('p', { class: 'fam-quiet' }, 'Nothing is playing right now.');

    F.clear(main);
    F.append(main, [
      pageHeader('pulse', 'Hi, ' + me.displayName, 'Your plan is ' + (ROLE_WORDS[me.role] || me.role) + '. Your space, your requests and what is playing, at a glance.', addButton('Add a device')),
      next,
      el('div', { class: 'fam-grid three' },
        card('Your space', 'Songs added for you.', null, meter(me.storageUsedBytes, limit, 'Your library'),
          limit > 0 && me.storageUsedBytes >= limit ? el('p', { class: 'fam-help fam-gap' }, 'Full, so new songs become requests.') : null),
        card('Requests this week', a.addToLibrary === 'SaveOnly' ? 'Your plan saves songs without asking for copies.' : 'Copies you asked for.', null,
          a.addToLibrary !== 'SaveOnly' ? countMeter(me.requestsThisWeek, a.weeklyRequestLimit, 'This week') : null,
          statusBadges.length ? el('div', { class: 'fam-badges fam-gap' }, statusBadges) : null),
        card('Playing now', devices.length + (devices.length === 1 ? ' device' : ' devices') + ' signed in.', null, nowPlaying)),
      card('What your plan allows', 'Set by whoever runs the family.', null, planLines(a))
    ]);
  }

  // ── You: Devices ─────────────────────────────────────────────────────────────────────
  function deviceKindWords(kind) {
    return { OctoApp: 'Octo app', SubsonicApp: 'App with an app password', NavidromeWeb: 'Navidrome in a browser', Detected: 'Seen by Octo' }[kind] || kind;
  }

  function deviceRow(device, options) {
    options = options || {};
    var playing = device.playing ? el('div', { class: 'fam-row-meta fam-playing' }, F.icon('radio'), el('span', null, 'Playing ', el('strong', null, device.playing.title), device.playing.artist ? ' by ' + device.playing.artist : '')) : null;
    var side = el('div', { class: 'fam-row-side' });
    if (options.quality) {
      var select = el('select', { class: 'set-input fam-select', 'aria-label': 'Quality for ' + (device.name || device.app), onchange: async function () {
        var reply = await api('PUT', '/me/devices/' + encodeURIComponent(device.id), { quality: select.value });
        if (!reply.ok) return F.toast(F.errorOf(reply), true);
        F.toast(select.value === 'Account' ? 'Follows your audio quality' : 'The app decides');
      } },
      el('option', { value: 'Account' }, 'Use my audio quality'),
      el('option', { value: 'App' }, 'Let this app decide'));
      select.value = device.quality || 'Account';
      side.appendChild(select);
    }
    if (options.signOut && device.kind !== 'Detected') {
      side.appendChild(el('button', { class: 'btn btn-sm btn-ghost btn-danger', type: 'button', onclick: async function () {
        if (!confirmIn('Sign out ' + (device.name || device.app || 'this device') + '? It stops playing at once, and needs a new code or app password to come back.')) return;
        var reply = await api('DELETE', options.signOut + encodeURIComponent(device.id));
        if (!reply.ok) return F.toast(F.errorOf(reply), true);
        F.toast('Signed out');
        if (options.after) options.after();
      } }, 'Sign out'));
    }
    return el('li', { class: 'fam-row stack' },
      el('span', { class: 'fam-row-ico', 'aria-hidden': 'true' }, F.icon(device.kind === 'NavidromeWeb' ? 'globe' : 'phone')),
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
    var devices = reply.data;
    var addButton = function (words) {
      return el('button', { class: 'btn btn-primary', type: 'button', onclick: function () { deviceDialog(null); } }, F.icon('plus'), words);
    };
    var list = el('ul', { class: 'fam-list' });
    devices.forEach(function (d) {
      list.appendChild(deviceRow(d, { quality: true, signOut: session.managed ? '/me/devices/' : null, after: function () { viewDevices(main); } }));
    });
    F.clear(main);
    F.append(main, [
      pageHeader('phone', 'Devices', session.managed
        ? 'Every app you signed in, where it was last seen and what it plays. Add one, choose its quality, or sign it out.'
        : 'The apps Octo has seen you use. Choose how each picks its audio quality.', session.managed ? addButton('Add a device') : null),
      card(null, null, null, devices.length ? list : empty('phone',
        session.managed ? 'No devices yet. The Octo app scans a code; any other app gets an app password.' : 'Octo has not seen a device of yours yet. Play something in any app and it shows up here.',
        session.managed ? addButton('Add your first device') : null))
    ]);
  }

  function viewIfShowing(view) {
    if (currentView() === view) route();
  }

  // ── You: library, saved, requests ────────────────────────────────────────────────────
  function cover(id, words) {
    var tile = el('div', { class: 'fam-cover', 'aria-hidden': 'true' }, F.initials(words));
    if (id) {
      var img = el('img', { class: 'fam-cover', alt: '', loading: 'lazy', src: '/api/family/cover/' + encodeURIComponent(id) + '?size=96' });
      img.addEventListener('error', function () { img.replaceWith(tile); });
      return img;
    }
    return tile;
  }

  async function viewLibrary(main) {
    var offset = 0;
    var size = 100;
    var list = el('ul', { class: 'fam-list' });
    var more = el('button', { class: 'btn', type: 'button', hidden: true }, 'Show more');
    var summary = el('div');
    var body = el('div', { class: 'fam-gap' }, list);
    var load = async function () {
      var reply = await api('GET', '/me/library?offset=' + offset + '&size=' + size);
      if (!reply.ok) return failed(main, reply);
      var page = reply.data;
      F.clear(summary).appendChild(meter(page.usedBytes, page.limitBytes, page.total + (page.total === 1 ? ' song' : ' songs')));
      if (!page.total) {
        F.clear(body).appendChild(empty('folder', 'Nothing here yet. Songs added for you, and copies you asked for, land in your library.',
          el('a', { class: 'btn', href: '#/saved' }, 'Open saved songs')));
      }
      page.songs.forEach(function (song) {
        var row = el('li', { class: 'fam-row stack' },
          cover(song.coverArt || song.id, song.title),
          el('div', { class: 'fam-row-main' },
            el('div', { class: 'fam-row-title' }, song.title),
            el('div', { class: 'fam-row-meta' }, [song.artist, song.album, F.bytes(song.size)].filter(Boolean).join(' · '))),
          el('div', { class: 'fam-row-side' }, el('button', { class: 'btn btn-sm btn-ghost', type: 'button', onclick: async function () {
            if (!confirmIn('Remove ' + song.title + ' from your library? It frees its space, and you can ask for it again later.')) return;
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
    F.append(main, [
      pageHeader('folder', 'My library', 'Songs added for you, and how much of your space they take. Remove what you no longer play.'),
      card(null, null, null, summary, body, el('div', { class: 'fam-actions' }, more))
    ]);
    await load();
  }

  async function viewSaved(main) {
    var reply = await api('GET', '/me/saved');
    if (!reply.ok) return failed(main, reply);
    var mayAsk = session.abilities && session.abilities.addToLibrary !== 'SaveOnly';
    var songs = el('ul', { class: 'fam-list' });
    var saved = reply.data;
    saved.songs.forEach(function (song) {
      var row = el('li', { class: 'fam-row stack' },
        cover(song.coverArt, song.title),
        el('div', { class: 'fam-row-main' },
          el('div', { class: 'fam-row-title' }, song.title),
          el('div', { class: 'fam-row-meta' }, [song.artist, song.album].filter(Boolean).join(' · '))),
        el('div', { class: 'fam-row-side' },
          mayAsk ? el('button', { class: 'btn btn-sm btn-primary', type: 'button', onclick: function () { askSheet(song); } }, 'Ask for a copy') : null,
          el('button', { class: 'btn btn-sm btn-ghost', type: 'button', onclick: async function () {
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
        el('div', { class: 'fam-row-side' }, el('button', { class: 'btn btn-sm btn-ghost', type: 'button', onclick: async function () {
          var r = await api('DELETE', '/me/saved/' + encodeURIComponent(album.id));
          if (!r.ok) return F.toast(F.errorOf(r), true);
          row.remove();
        } }, 'Remove')));
      songs.appendChild(row);
    });
    F.clear(main);
    F.append(main, [
      pageHeader('heart', 'Saved', 'Songs and albums you hearted from outside the library. They play from where Octo found them' + (mayAsk ? ', and you can ask for a copy of your own.' : '.')),
      card(null, null, null, saved.songs.length || saved.albums.length ? songs
        : empty('heart', 'Nothing saved yet. Heart a song Octo found outside the library, in any app, and it shows up here.'))
    ]);
  }

  function askSheet(song) {
    var max = (session.abilities && session.abilities.requestQuality) || 'Best';
    var rank = { Mp3: 0, Flac: 1, Best: 2 };
    var options = [['Best', 'Best there is'], ['Flac', 'FLAC'], ['Mp3', 'MP3']].filter(function (o) { return rank[o[0]] <= rank[max]; });
    var quality = options[0][0];
    var send = el('button', { class: 'btn btn-primary', type: 'button', onclick: async function () {
      var reply = await api('POST', '/me/requests', { id: song.id, quality: quality });
      if (!reply.ok) return F.toast(F.errorOf(reply), true);
      F.closeSheet();
      F.toast(reply.data.state === 'Done' ? 'It is in your library' : reply.data.state === 'Approved' ? 'Approved' : 'Asked. You will hear back.');
    } }, 'Ask for a copy');
    F.openSheet('Ask for a copy', el('div', { class: 'fam-form' },
      el('div', { class: 'fam-label' }, 'Quality', segmented(options, quality, function (v) { quality = v; }, 'Quality'))),
    { sub: song.title + (song.artist ? ' by ' + song.artist : ''), foot: [el('span'), send] });
  }

  function requestRow(request, options) {
    options = options || {};
    var side = el('div', { class: 'fam-row-side' });
    var note = el('input', { class: 'set-input', placeholder: 'A note for them (optional)', 'aria-label': 'Note for ' + (request.displayName || request.username || 'them') });
    if (options.decide && request.state === 'Pending') {
      side.appendChild(el('button', { class: 'btn btn-sm btn-primary', type: 'button', onclick: function () { options.decide(request, true, note.value); } }, 'Approve'));
      side.appendChild(el('button', { class: 'btn btn-sm btn-ghost', type: 'button', onclick: function () { options.decide(request, false, note.value); } }, 'Decline'));
    }
    if (options.cancel && request.state === 'Pending') {
      side.appendChild(el('button', { class: 'btn btn-sm btn-ghost', type: 'button', onclick: function () { options.cancel(request); } }, 'Cancel'));
    }
    if (options.retry && request.state === 'Failed' && request.quality === 'Flac') {
      side.appendChild(el('button', { class: 'btn btn-sm', type: 'button', onclick: function () { options.retry(request); } }, 'Try MP3'));
    }
    var meta = [request.artist, request.album].filter(Boolean).join(' · ');
    var who = options.decide ? request.displayName + ' asked ' + F.ago(request.created).toLowerCase() : 'Asked ' + F.ago(request.created).toLowerCase();
    var warning = options.fullFor && options.fullFor(request) ? el('div', { class: 'fam-row-meta fam-warn-text' }, options.fullFor(request)) : null;
    return el('li', { class: 'fam-row stack' },
      cover(request.coverArt, request.title),
      el('div', { class: 'fam-row-main' },
        el('div', { class: 'fam-row-title' }, request.title || request.target),
        meta ? el('div', { class: 'fam-row-meta' }, meta) : null,
        el('div', { class: 'fam-row-meta' }, who),
        el('div', { class: 'fam-badges' }, stateBadge(request.state), badge(qualityWord(request.quality))),
        request.note ? el('div', { class: 'fam-row-meta' }, '“' + request.note + '”') : null,
        request.failure ? el('div', { class: 'fam-row-meta fam-bad-text' }, request.failure) : null,
        warning,
        options.decide && request.state === 'Pending' ? el('div', { class: 'fam-note-field' }, note) : null),
      side);
  }

  async function viewRequests(main) {
    var reply = await api('GET', '/me/requests');
    if (!reply.ok) return failed(main, reply);
    var list = el('ul', { class: 'fam-list' });
    reply.data.forEach(function (request) {
      list.appendChild(requestRow(request, { cancel: async function (r) {
        if (!confirmIn('Cancel your request for ' + (r.title || 'this song') + '?')) return;
        var answer = await api('DELETE', '/me/requests/' + encodeURIComponent(r.id));
        if (!answer.ok) return F.toast(F.errorOf(answer), true);
        F.toast('Canceled');
        viewRequests(main);
      }, retry: async function (r) {
        var answer = await api('POST', '/me/requests', { id: r.target, quality: 'Mp3' });
        if (!answer.ok) return F.toast(F.errorOf(answer), true);
        F.toast(answer.data.state === 'Pending' ? 'Asked for an MP3. You will hear back.' : 'Asked for an MP3');
        viewRequests(main);
      } }));
    });
    F.clear(main);
    F.append(main, [
      pageHeader('tray', 'Requests', 'Copies you asked for, and what became of them. Cancel one that is still waiting.'),
      card(null, null, null, reply.data.length ? list
        : empty('tray', 'No requests yet. Ask for a copy of a saved song, here or in the Octo app.', el('a', { class: 'btn', href: '#/saved' }, 'Open saved songs')))
    ]);
  }

  // ── You: Audio quality and Account ───────────────────────────────────────────────────
  async function viewQuality(main) {
    var reply = await api('GET', '/me');
    if (!reply.ok) return failed(main, reply);
    var q = reply.data.quality;
    var help = el('p', { class: 'fam-help' });
    var save = async function (place, value) {
      var body = {};
      body[place] = value;
      var r = await api('PUT', '/me/quality', body);
      if (!r.ok) return F.toast(F.errorOf(r), true);
      q = r.data;
      F.toast(place === 'home' ? 'At home: ' + value : 'Away: ' + value);
      describe();
    };
    var describe = function () {
      var lines = ['At home: ' + QUALITY_HELP[q.home] + ' Away: ' + QUALITY_HELP[q.away]];
      if (q.familyLimitKbps > 0) lines.push('Your family plan limits listening to ' + q.familyLimitKbps + ' kbps.');
      if (q.familyAwayLimitKbps > 0) lines.push('Your family plan limits away listening to ' + q.familyAwayLimitKbps + ' kbps.');
      help.textContent = lines.join(' ');
    };
    describe();
    F.clear(main);
    F.append(main, [
      pageHeader('sliders', 'Audio quality', 'What Octo sends your devices, at home and away. Each device can follow this or let its app decide, under Devices.'),
      rowsCard([
        setRow('At home', 'On your home network.', segmented(QUALITIES, q.home, function (v) { save('home', v); }, 'Quality at home')),
        setRow('Away from home', 'Everywhere else, often on mobile data.', segmented(QUALITIES, q.away, function (v) { save('away', v); }, 'Quality away from home')),
        el('div', { class: 'set-row' }, help)
      ]),
      el('p', { class: 'fam-help fam-gap' }, 'Songs from outside the library already stream at about 128 to 160 kbps, so these choices leave them as they are.')
    ]);
  }

  async function viewAccount(main) {
    var reply = await api('GET', '/me');
    if (!reply.ok) return failed(main, reply);
    F.clear(main);
    F.append(main, [
      pageHeader('key', 'Account', 'Your password for this page, your Last.fm scrobbles and where your notifications go.'),
      passwordCard(),
      lastFmCard(),
      ntfyCard(reply.data)
    ]);
  }

  function randomTopic() {
    var letters = 'abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789';
    var bytes = new Uint8Array(24);
    crypto.getRandomValues(bytes);
    return Array.prototype.map.call(bytes, function (b) { return letters.charAt(b % letters.length); }).join('');
  }

  function ntfyCard(me) {
    var input = el('input', { class: 'set-input fam-mono', value: me.ntfyTopic || '', placeholder: 'A private topic name', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'ntfy topic' });
    var save = async function () {
      var reply = await api('PUT', '/me/ntfy', { topic: input.value.trim() });
      if (!reply.ok) return F.toast(F.errorOf(reply), true);
      F.toast(input.value.trim() ? 'Pushes go to this topic now' : 'Pushes are off');
    };
    return card('Notifications', 'A push when a request is approved, declined, added or fails. Install the ntfy app and subscribe to your topic.', null,
      el('div', { class: 'fam-form' },
        el('div', { class: 'set-inline' }, input),
        el('div', { class: 'fam-actions' },
          el('button', { class: 'btn btn-ghost', type: 'button', onclick: function () { input.value = randomTopic(); } }, 'Suggest a private topic'),
          el('button', { class: 'btn btn-primary', type: 'button', onclick: save }, 'Save')),
        el('p', { class: 'fam-help' }, 'Anyone who knows a topic name can read it, so use a long random one. Leave it empty for no pushes.')));
  }

  function lastFmCard() {
    var body = el('div', { class: 'fam-form' }, el('p', { class: 'fam-quiet' }, 'Loading'));
    var node = card('Last.fm', 'Send what you play to your own Last.fm account.', null, body);
    var load = async function () {
      var reply = await api('GET', '/me/lastfm');
      F.clear(body);
      if (!reply.ok) return body.appendChild(el('p', { class: 'field-error' }, F.errorOf(reply)));
      var info = reply.data;
      if (!info.available) {
        return body.appendChild(el('p', { class: 'fam-quiet' }, 'Last.fm is not set up on this server yet. Ask whoever runs it to add a Last.fm API key.'));
      }
      var user = info.user;
      if (user && user.connected) {
        body.appendChild(el('p', null, 'Connected as ', el('strong', null, user.lastFmUser || 'your account'), '.'));
        body.appendChild(el('div', null, el('button', { class: 'btn btn-ghost', type: 'button', onclick: async function () {
          if (!confirmIn('Disconnect Last.fm? What you play stops being sent.')) return;
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
      body.appendChild(el('div', { class: 'fam-actions' },
        el('button', { class: 'btn btn-primary', type: 'button', onclick: async function () {
          var r = await api('POST', '/me/lastfm/connect', {});
          if (!r.ok) return F.toast(F.errorOf(r), true);
          window.open(r.data.url, '_blank', 'noopener');
        } }, 'Connect'),
        el('button', { class: 'btn btn-ghost', type: 'button', onclick: async function () {
          var r = await api('POST', '/me/lastfm/finish', {});
          if (!r.ok) return F.toast(r.status === 409 ? 'Last.fm has not seen your approval yet.' : F.errorOf(r), true);
          F.toast('Connected');
          load();
        } }, 'Finish')));
    };
    load();
    return node;
  }

  function passwordCard() {
    var current = el('input', { class: 'set-input', type: 'password', autocomplete: 'current-password', id: 'pw-current' });
    var next = el('input', { class: 'set-input', type: 'password', autocomplete: 'new-password', id: 'pw-next', minlength: 8 });
    var error = el('p', { class: 'field-error', role: 'alert' });
    var form = el('form', { class: 'fam-form', onsubmit: async function (e) {
      e.preventDefault();
      error.textContent = '';
      var reply = await api('PUT', '/me/password', { current: current.value, next: next.value });
      if (!reply.ok) { error.textContent = F.errorOf(reply); return; }
      current.value = '';
      next.value = '';
      F.toast('Password changed');
    } },
    el('div', { class: 'set-grid2' },
      el('label', { class: 'fam-label', for: 'pw-current' }, 'Current password', current),
      el('label', { class: 'fam-label', for: 'pw-next' }, 'New password', next, el('span', { class: 'fam-help' }, 'At least 8 characters.'))),
    error,
    el('div', null, el('button', { class: 'btn btn-primary', type: 'submit' }, 'Change password')));
    return card('Password', 'For this page and Navidrome in a browser. Your apps use their own passwords, so they stay signed in.', null, form);
  }

  // ── The outside address ──────────────────────────────────────────────────────────────
  // Where the dashboard keeps it: its Status page, at the public address field.
  var STATUS_ADDRESS = '/admin/#status/public-address';

  // The owner's inline field for the outside address, with Save and the reachability check
  // in plain words. `onSaved(publicUrl, words)` runs after a save.
  function addressField(onSaved) {
    var input = el('input', { class: 'set-input fam-mono', type: 'text', inputmode: 'url', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'none',
      placeholder: 'https://music.example.com', 'aria-label': 'Outside address', value: session.publicUrl || session.suggestedPublicUrl || '' });
    var result = el('p', { class: 'inline-note', role: 'status', hidden: true });
    var save = el('button', { class: 'btn btn-primary', type: 'submit' }, 'Save');
    var form = el('form', { class: 'fam-address', onsubmit: async function (e) {
      e.preventDefault();
      save.disabled = true;
      result.hidden = false;
      result.className = 'inline-note busy';
      result.textContent = 'Saving';
      var r = await api('PUT', '/public-address', { url: input.value.trim() });
      if (!r.ok) {
        save.disabled = false;
        result.className = 'inline-note error';
        result.textContent = F.errorOf(r);
        return;
      }
      session.publicUrl = r.data.publicUrl;
      var words = 'Saved.';
      if (r.data.publicUrl) {
        result.textContent = 'Saved. Checking it from here';
        var check = await api('POST', '/public-address/check', {});
        words = check.ok ? 'Saved. ' + check.data.message : 'Saved.';
        result.className = 'inline-note ' + (check.ok && check.data.reachable ? 'ok' : 'info');
      } else {
        result.className = 'inline-note ok';
      }
      result.textContent = words;
      save.disabled = false;
      if (onSaved) onSaved(r.data.publicUrl, words);
    } },
    el('div', { class: 'set-inline' }, input, save),
    session.suggestedPublicUrl && !session.publicUrl ? el('p', { class: 'fam-help' }, 'Octo saw your reverse proxy use ', el('code', null, session.suggestedPublicUrl), '. Check it is right, then save.') : null,
    result);
    return form;
  }

  // The notice when links only work at home: what it means, the dashboard's control, and the
  // inline field for the owner.
  function addressNotice(onSaved) {
    return el('section', { class: 'notice notice-warn fam-address-notice' },
      el('p', null, el('strong', null, 'Set your outside address.'), ' Without it, invite links and device codes only work on your home network.'),
      session.owner
        ? el('div', { class: 'fam-stack' },
          el('div', null, el('a', { class: 'btn btn-sm', href: STATUS_ADDRESS }, F.icon('out'), 'Set it on the Status page')),
          el('p', { class: 'fam-help' }, 'Or type it here:'),
          addressField(onSaved))
        : el('p', { class: 'fam-help' }, 'Ask whoever runs this server to set its outside address.'));
  }

  // ── Family: Members ──────────────────────────────────────────────────────────────────
  async function viewMembers(main) {
    if (!session.familyEnabled) return viewFamilyOff(main);
    var replies = await Promise.all([api('GET', '/members'), api('GET', '/unrated'), api('GET', '/devices')]);
    if (!replies[0].ok) return failed(main, replies[0]);
    var members = replies[0].data;
    var devices = replies[2].ok ? replies[2].data : [];
    var addButton = function (words) {
      return el('button', { class: 'btn btn-primary', type: 'button', onclick: addMemberSheet }, F.icon('plus'), words);
    };
    var parts = [pageHeader('users', 'Members', 'Everyone you share your music with. Add someone, send them their link, and choose what they can do.', addButton('Add member'))];
    if (!session.publicUrl) parts.push(addressNotice(function () { route(); }));
    if (session.links === 'Copy' && session.linksMessage) {
      parts.push(el('div', { class: 'notice notice-warn', role: 'status' }, session.linksMessage));
    }
    if (session.proxyWarning) {
      parts.push(el('div', { class: 'notice notice-warn', role: 'alert' }, session.proxyWarning));
    }
    var unrated = replies[1].ok ? replies[1].data : null;
    if (unrated && unrated.count > 0) {
      var mark = el('button', { class: 'btn btn-sm', type: 'button', onclick: async function () {
        mark.disabled = true;
        var r = await api('POST', '/unrated/mark', {});
        mark.disabled = false;
        F.toast(r.ok ? r.data.message : F.errorOf(r), !r.ok);
      } }, 'Mark them now');
      parts.push(el('div', { class: 'notice fam-notice-row' },
        el('div', null,
          el('strong', null, unrated.count + (unrated.count === 1 ? ' song has' : ' songs have') + ' no explicit mark. '),
          'Kid profiles cannot tell whether those are clean. Octo can look them up and mark them.'),
        mark));
    }
    if (!members.length) {
      parts.push(card(null, null, null, empty('users', 'No one yet. Add the first member of your family: they get a link to set their password, then add their own devices.',
        addButton('Add the first member'))));
    } else {
      parts.push(el('div', { class: 'fam-grid two' }, members.map(function (m) {
        return memberCard(m, devices.filter(function (d) { return d.username.toLowerCase() === m.username.toLowerCase(); }));
      })));
    }
    F.clear(main);
    F.append(main, parts);
  }

  async function viewFamilyOff(main) {
    var parts = [pageHeader('users', 'Family is off', 'Family gives the people you share your music with their own sign-in, library and devices.')];
    if (session.role === 'Owner') {
      var report = el('div');
      var button = el('button', { class: 'btn btn-primary', type: 'button', onclick: async function () {
        button.disabled = true;
        var r = await api('POST', '/enable', { enabled: true });
        button.disabled = false;
        if (!r.ok) { F.clear(report).appendChild(el('p', { class: 'notice notice-error' }, F.errorOf(r))); return; }
        F.toast('Family is on');
        start();
      } }, 'Turn on Family');
      parts.push(card('Turn it on', null, null,
        el('p', null, 'Octo makes a folder for each member at ', el('code', null, session.membersPath), '. Navidrome needs to see that folder at the same path.'),
        el('div', { class: 'fam-actions' }, button),
        report));
    } else {
      parts.push(card(null, null, null, el('p', null, 'Ask whoever runs this server to turn Family on.')));
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
    var more = el('details', { class: 'fam-more' },
      el('summary', { class: 'btn btn-sm btn-ghost', 'aria-label': 'More for ' + m.displayName }, 'More', F.icon('caretDown')),
      el('div', { class: 'fam-more-menu' },
        el('button', { type: 'button', onclick: function () { suspend(m); } }, m.suspended ? 'Resume listening' : 'Pause listening'),
        el('button', { type: 'button', onclick: function () { repair(m); } }, 'Repair their account'),
        el('button', { type: 'button', class: 'fam-danger', onclick: function () { removeSheet(m); } }, 'Remove from the family')));
    return el('section', { class: 'info-card fam-card fam-member' },
      el('div', { class: 'fam-member-head' },
        F.avatar(m.displayName, 'fam-avatar-lg'),
        el('div', { class: 'fam-row-main' },
          el('h2', { class: 'fam-member-name' }, m.displayName),
          el('div', { class: 'fam-row-meta' }, m.username + ' · ' + m.devices + (m.devices === 1 ? ' device' : ' devices')),
          el('div', { class: 'fam-badges' }, badges))),
      meter(m.storageUsedBytes, limit, 'Their library'),
      el('div', { class: 'fam-actions' },
        el('button', { class: 'btn btn-sm btn-primary', type: 'button', onclick: function () { editMemberSheet(m); } }, 'Edit'),
        el('button', { class: 'btn btn-sm', type: 'button', onclick: function () { memberDevicesSheet(m, devices); } }, 'Devices'),
        el('button', { class: 'btn btn-sm', type: 'button', onclick: function () { inviteDialog(m, null); } }, m.joined ? 'New sign-in link' : 'Send their link'),
        more));
  }

  function refreshFamily() {
    if (currentView() === 'members') route();
  }

  async function suspend(m) {
    if (!m.suspended && !confirmIn('Pause ' + m.displayName + '? Their devices stop playing until you resume them.')) return;
    var r = await api('PUT', '/members/' + encodeURIComponent(m.username), { suspended: !m.suspended });
    if (!r.ok) return F.toast(F.errorOf(r), true);
    F.toast(r.data.suspended ? m.displayName + ' is paused' : m.displayName + ' can listen again');
    refreshFamily();
  }

  async function repair(m) {
    var r = await api('POST', '/members/' + encodeURIComponent(m.username) + '/repair', {});
    F.toast(r.ok ? 'Repaired ' + m.displayName + '’s account' : F.errorOf(r), !r.ok);
  }

  function memberDevicesSheet(m, devices) {
    var list = el('ul', { class: 'fam-list' });
    devices.forEach(function (d) {
      list.appendChild(deviceRow(d, { signOut: '/devices/', after: function () { F.closeSheet(); refreshFamily(); } }));
    });
    var add = m.suspended ? null : el('button', { class: 'btn btn-primary', type: 'button', onclick: function () { deviceDialog(m); } }, F.icon('plus'), 'Add a device for ' + m.displayName);
    F.openSheet(m.displayName + '’s devices', devices.length ? list : empty('phone', 'No devices yet. Add one for them, or send their link so they add their own.'),
      { sub: 'Sign one out, or add one for them.', foot: [el('span'), add] });
  }

  async function librariesPicker(current) {
    var reply = await api('GET', '/libraries');
    var shared = reply.ok ? reply.data.filter(function (l) { return !l.personal; }) : [];
    var all = current === 'All' || current === undefined;
    var chosen = Array.isArray(current) ? current.slice() : [];
    var box = el('div', { class: 'set-card fam-rows' });
    var allSwitch = switchControl(all, 'Every shared library');
    var ids = el('div', { hidden: all });
    shared.forEach(function (l) {
      var check = switchControl(chosen.indexOf(l.id) >= 0 || all, l.name);
      check.input.setAttribute('data-id', l.id);
      ids.appendChild(setRow(l.name, l.path, check.node));
    });
    allSwitch.input.addEventListener('change', function () { ids.hidden = allSwitch.input.checked; });
    box.appendChild(setRow('Every shared library', 'Libraries you add later are shared too.', allSwitch.node));
    box.appendChild(ids);
    box.value = function () {
      if (allSwitch.input.checked) return 'All';
      return Array.prototype.filter.call(ids.querySelectorAll('input'), function (i) { return i.checked; })
        .map(function (i) { return Number(i.getAttribute('data-id')); });
    };
    return box;
  }

  async function addMemberSheet() {
    var preset = 'Listener';
    var username = el('input', { class: 'set-input', id: 'new-username', autocapitalize: 'none', spellcheck: 'false', autocomplete: 'off', placeholder: 'alex', required: true });
    var displayName = el('input', { class: 'set-input', id: 'new-name', autocomplete: 'off', placeholder: 'Alex' });
    var presetHelp = el('p', { class: 'fam-help' }, PRESETS[preset]);
    var presets = [['Listener', 'Listener'], ['Member', 'Member'], ['Kid', 'Kid']];
    if (session.role === 'Owner') presets.push(['CoAdmin', 'Co-admin']);
    var away = switchControl(true, 'Allow listening away from home');
    var error = el('p', { class: 'field-error', role: 'alert' });
    var libraries = await librariesPicker('All');
    var adopt = false;
    var submit = el('button', { class: 'btn btn-primary', type: 'submit', form: 'add-member' }, 'Add');
    var form = el('form', { class: 'fam-form', id: 'add-member', onsubmit: async function (e) {
      e.preventDefault();
      error.textContent = '';
      submit.disabled = true;
      var r = await api('POST', '/members', {
        username: username.value.trim().toLowerCase(),
        displayName: displayName.value.trim(),
        preset: preset,
        sharedLibraries: libraries.value(),
        adopt: adopt,
        away: away.input.checked
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
      inviteDialog(r.data.member, r.data);
      refreshFamily();
    } },
    el('div', { class: 'set-grid2' },
      el('label', { class: 'fam-label', for: 'new-name' }, 'Name', displayName, el('span', { class: 'fam-help' }, 'How the family sees them.')),
      el('label', { class: 'fam-label', for: 'new-username' }, 'Username', username, el('span', { class: 'fam-help' }, 'Lowercase letters, numbers, dots, dashes or underscores.'))),
    el('div', { class: 'fam-label' }, 'What they can do', segmented(presets, preset, function (v) { preset = v; presetHelp.textContent = PRESETS[v]; }, 'Plan'), presetHelp),
    rowsCard([setRow('Allow listening away from home', 'On by default. Off keeps their links and apps to your home network.', away.node)]),
    el('div', { class: 'fam-label' }, 'Shared music they see', libraries),
    error);
    F.openSheet('Add member', form, { side: true, sub: 'They get a link to set their password, then add their own devices.', foot: [el('button', { class: 'btn btn-ghost', type: 'button', onclick: F.closeSheet }, 'Cancel'), submit] });
    displayName.focus();
  }

  async function editMemberSheet(m) {
    var preset = m.preset;
    var values = Object.assign({}, m.abilities);
    var display = el('input', { class: 'set-input', id: 'edit-name', value: m.displayName });
    var fields = el('div', { class: 'fam-stack' });
    var libraries = await librariesPicker(m.sharedLibraries);
    var presetHelp = el('p', { class: 'fam-help' }, PRESETS[preset]);
    var drawFields = function () {
      F.clear(fields);
      ABILITY_GROUPS.forEach(function (group) {
        if (group.owner && session.role !== 'Owner') return;
        var rows = group.fields.map(function (f) {
          var control;
          if (f.kind === 'switch') {
            var sw = switchControl(values[f.key], f.title, function () { values[f.key] = sw.input.checked; });
            control = sw.node;
          } else if (f.kind === 'number') {
            control = el('input', { class: 'set-input fam-number', type: 'number', min: 0, step: 1, value: values[f.key], inputmode: 'numeric', 'aria-label': f.title, oninput: function () { values[f.key] = Math.max(0, parseInt(control.value, 10) || 0); } });
          } else {
            control = el('select', { class: 'set-input fam-select', 'aria-label': f.title, onchange: function () { values[f.key] = control.value; } },
              f.options.map(function (o) { return el('option', { value: o[0] }, o[1]); }));
            control.value = values[f.key];
          }
          return setRow(f.title, f.help, control);
        });
        fields.appendChild(el('div', { class: 'fam-group' }, el('h3', { class: 'fam-group-title' }, group.title), rowsCard(rows)));
      });
    };
    drawFields();
    var presets = [['Listener', 'Listener'], ['Member', 'Member'], ['Kid', 'Kid']];
    if (session.role === 'Owner' || m.preset === 'CoAdmin') presets.push(['CoAdmin', 'Co-admin']);
    var error = el('p', { class: 'field-error', role: 'alert' });
    var save = el('button', { class: 'btn btn-primary', type: 'submit', form: 'edit-member' }, 'Save');
    var form = el('form', { class: 'fam-form', id: 'edit-member', onsubmit: async function (e) {
      e.preventDefault();
      var base = PRESET_ABILITIES[preset] || {};
      var edits = {};
      ABILITY_FIELDS.forEach(function (f) {
        if (values[f.key] !== base[f.key]) edits[f.key] = values[f.key];
      });
      save.disabled = true;
      var r = await api('PUT', '/members/' + encodeURIComponent(m.username), {
        displayName: display.value.trim(),
        preset: preset,
        edits: edits,
        sharedLibraries: libraries.value()
      });
      save.disabled = false;
      if (!r.ok) { error.textContent = F.errorOf(r); return; }
      F.closeSheet();
      F.toast('Saved');
      refreshFamily();
    } },
    el('label', { class: 'fam-label', for: 'edit-name' }, 'Name', display),
    el('div', { class: 'fam-label' }, 'Start from a plan', segmented(presets, preset, function (v) {
      preset = v;
      presetHelp.textContent = PRESETS[v];
      values = Object.assign({}, PRESET_ABILITIES[v]);
      drawFields();
    }, 'Plan'), presetHelp),
    fields,
    el('div', { class: 'fam-group' }, el('h3', { class: 'fam-group-title' }, 'Shared music they see'), libraries),
    el('p', { class: 'fam-help' }, 'Outside songs stream at about 128 to 160 kbps whatever the limits say.'),
    error);
    F.openSheet('Edit ' + m.displayName, form, { side: true, sub: 'Choose a plan, then change any ability. Each one says what it does.', foot: [el('button', { class: 'btn btn-ghost', type: 'button', onclick: F.closeSheet }, 'Cancel'), save] });
  }

  function removeSheet(m) {
    var files = 'keep';
    var confirmInput = el('input', { class: 'set-input', id: 'confirm-name', autocapitalize: 'none', spellcheck: 'false', autocomplete: 'off' });
    var error = el('p', { class: 'field-error', role: 'alert' });
    var button = el('button', { class: 'btn btn-primary btn-destructive', type: 'submit', form: 'remove-member', disabled: true }, 'Remove ' + m.displayName);
    confirmInput.addEventListener('input', function () { button.disabled = confirmInput.value.trim().toLowerCase() !== m.username.toLowerCase(); });
    var choices = [
      ['keep', 'Keep them aside', 'Songs only they had are kept in a folder for you.'],
      ['promote', 'Add to shared', 'Everyone can play them.'],
      ['delete', 'Delete them', 'Songs only they had go to Octo’s trash.']
    ];
    var help = el('p', { class: 'fam-help' }, choices[0][2]);
    var form = el('form', { class: 'fam-form', id: 'remove-member', onsubmit: async function (e) {
      e.preventDefault();
      button.disabled = true;
      var r = await api('DELETE', '/members/' + encodeURIComponent(m.username), { confirm: confirmInput.value.trim(), files: files });
      button.disabled = false;
      if (!r.ok) { error.textContent = F.errorOf(r); return; }
      F.closeSheet();
      F.toast(m.displayName + ' was removed');
      refreshFamily();
    } },
    el('div', { class: 'fam-label' }, 'Their songs', segmented(choices.map(function (c) { return [c[0], c[1]]; }), files, function (v) {
      files = v;
      help.textContent = choices.filter(function (c) { return c[0] === v; })[0][2];
    }, 'Their songs'), help),
    el('label', { class: 'fam-label', for: 'confirm-name' }, 'Type ' + m.username + ' to confirm', confirmInput),
    error);
    F.openSheet('Remove ' + m.displayName + '?', form, { sub: 'Their account, devices and own library are removed. Shared music is not touched.', foot: [el('button', { class: 'btn btn-ghost', type: 'button', onclick: F.closeSheet }, 'Cancel'), button] });
    confirmInput.focus();
  }

  // ── Family: requests to approve, all devices, playlists, activity ────────────────────
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
    F.append(main, [
      pageHeader('checkCircle', 'Requests to approve', 'Copies family members asked for. Approve or decline, with a note if you like; they hear what you decide.'),
      el('div', { class: 'fam-toolbar' }, filters),
      card(null, null, null, replies[0].data.length ? list
        : empty('checkCircle', filter === 'Pending' ? 'Nothing is waiting for you. New requests show up here, and in your notifications.' : 'Nothing here.'))
    ]);
  }

  async function viewAllDevices(main) {
    var reply = await api('GET', '/devices');
    if (!reply.ok) return failed(main, reply);
    var list = el('ul', { class: 'fam-list' });
    reply.data.forEach(function (d) {
      list.appendChild(deviceRow(d, { owner: true, signOut: '/devices/', after: function () { viewAllDevices(main); } }));
    });
    F.clear(main);
    F.append(main, [
      pageHeader('plugs', 'All devices', 'Every family member’s devices, and the apps Octo has seen for everyone else. Sign out any that should not be there.'),
      card(null, null, null, reply.data.length ? list : empty('plugs', 'No devices yet. They show up here as members add them.', el('a', { class: 'btn', href: '#/members' }, 'Open members')))
    ]);
  }

  async function viewPlaylists(main) {
    var reply = await api('GET', '/playlists');
    if (!reply.ok) return failed(main, reply);
    var list = el('ul', { class: 'fam-list' });
    reply.data.forEach(function (p) {
      var mine = p.createdBy && p.createdBy.toLowerCase() === session.username.toLowerCase();
      var side = el('div', { class: 'fam-row-side' });
      if (session.manager) {
        var approve = switchControl(p.approveAdditions, 'Approve additions to ' + p.name, async function () {
          var r = await api('PUT', '/playlists/' + encodeURIComponent(p.id), { approveAdditions: approve.input.checked });
          if (!r.ok) { approve.input.checked = !approve.input.checked; return F.toast(F.errorOf(r), true); }
          F.toast(approve.input.checked ? 'Additions wait for approval' : 'Anyone can add');
        });
        side.appendChild(el('span', { class: 'fam-inline-switch' }, el('span', { class: 'fam-row-meta' }, 'Approve additions'), approve.node));
      }
      if (session.manager || mine) {
        side.appendChild(el('button', { class: 'btn btn-sm btn-ghost', type: 'button', onclick: async function () {
          var name = window.prompt('New name', p.name);
          if (!name || !name.trim()) return;
          var r = await api('PUT', '/playlists/' + encodeURIComponent(p.id), { name: name.trim() });
          if (!r.ok) return F.toast(F.errorOf(r), true);
          viewPlaylists(main);
        } }, 'Rename'));
      }
      if (session.manager) {
        side.appendChild(el('button', { class: 'btn btn-sm btn-ghost btn-danger', type: 'button', onclick: async function () {
          if (!confirmIn('Delete the family playlist ' + p.name + '? The songs stay in the library.')) return;
          var r = await api('DELETE', '/playlists/' + encodeURIComponent(p.id));
          if (!r.ok) return F.toast(F.errorOf(r), true);
          viewPlaylists(main);
        } }, 'Delete'));
      }
      list.appendChild(el('li', { class: 'fam-row stack' },
        el('span', { class: 'fam-row-ico', 'aria-hidden': 'true' }, F.icon('playlist')),
        el('div', { class: 'fam-row-main' },
          el('div', { class: 'fam-row-title' }, p.name),
          el('div', { class: 'fam-row-meta' }, p.songCount + (p.songCount === 1 ? ' song' : ' songs') + ' · made by ' + p.createdBy)),
        side));
    });
    var name = el('input', { class: 'set-input', placeholder: 'Road trip', 'aria-label': 'Playlist name' });
    var canEdit = !session.abilities || session.abilities.familyPlaylistsEdit;
    var create = canEdit ? el('form', { class: 'set-inline fam-create', onsubmit: async function (e) {
      e.preventDefault();
      if (!name.value.trim()) { name.focus(); return; }
      var r = await api('POST', '/playlists', { name: name.value.trim() });
      if (!r.ok) return F.toast(F.errorOf(r), true);
      F.toast('Made ' + r.data.name);
      viewPlaylists(main);
    } }, name, el('button', { class: 'btn btn-primary', type: 'submit' }, F.icon('plus'), 'Make a playlist')) : null;
    F.clear(main);
    F.append(main, [
      pageHeader('playlist', 'Family playlists', 'Playlists everyone in the family sees and can add to from any app.'),
      card(null, null, null, create,
        reply.data.length ? list : empty('playlist', canEdit ? 'No family playlists yet. Name one above, then add songs to it from any app.' : 'No family playlists yet.'))
    ]);
  }

  function stat(n, words) {
    return el('div', { class: 'fam-stat' }, el('span', { class: 'fam-stat-n' }, n), el('span', { class: 'fam-row-meta' }, words));
  }

  async function viewActivity(main) {
    var reply = await api('GET', '/load');
    if (!reply.ok) return failed(main, reply);
    var load = reply.data;
    var streams = el('ul', { class: 'fam-list' });
    load.streams.forEach(function (s) {
      streams.appendChild(el('li', { class: 'fam-row' },
        el('span', { class: 'fam-row-ico', 'aria-hidden': 'true' }, F.icon('radio')),
        el('div', { class: 'fam-row-main' },
          el('div', { class: 'fam-row-title' }, s.songTitle || 'A song'),
          el('div', { class: 'fam-row-meta' }, s.username + ' · ' + s.device)),
        el('div', { class: 'fam-row-side' }, placeBadge(s.place), badge(s.kbps ? s.kbps + ' kbps' : 'Original'))));
    });
    var queue = el('ul', { class: 'fam-list' });
    load.queue.forEach(function (q) {
      queue.appendChild(el('li', { class: 'fam-row' }, el('div', { class: 'fam-row-main' }, el('div', { class: 'fam-row-title' }, q.username)), el('div', { class: 'fam-row-side' }, badge(q.waiting + ' waiting'))));
    });
    // The helper's slots, or a plain note when it did not answer (never zeros it did not say).
    var shim = load.shim;
    var helper = shim
      ? [stat(String(shim.active), 'Outside songs streaming'), stat(shim.limit ? shim.ytdlpRunning + ' of ' + shim.limit : String(shim.ytdlpRunning), 'Lookups running')]
      : el('div', { class: 'fam-stat' }, el('strong', null, 'The YouTube helper did not answer'), el('span', { class: 'fam-row-meta' }, 'Outside songs and lookups'));
    F.clear(main);
    F.append(main, [
      pageHeader('drives', 'Activity', 'How busy the server is right now: who is playing, what is downloading, and how much is going out.'),
      card(null, null, null, el('div', { class: 'fam-stats' },
        stat(String(load.streams.length), 'Streams playing'),
        helper,
        stat(load.uploadKbps + ' kbps', 'Sent out now'))),
      el('div', { class: 'fam-grid two' },
        card('Playing now', null, null, load.streams.length ? streams : empty('radio', 'Nothing is playing right now.')),
        card('Downloads waiting', null, null, load.queue.length ? queue : empty('tray', 'No downloads waiting.')))
    ]);
  }

  // ── The add a device and invite dialogs ──────────────────────────────────────────────
  // Both share one shape: who it is for and how long it works at the top; the link choice
  // (anywhere or at home); the QR code on a raised panel on the left, and the other ways to
  // use it, numbered, on the right (stacked on narrow screens); and a footer with Done.

  // A code is renewed this long before it runs out, a failed renewal is tried again after
  // REFRESH_RETRY, and renewing stops once a dialog has been open for REFRESH_FOR.
  var REFRESH_LEAD = 5000;
  var REFRESH_RETRY = 10000;
  var REFRESH_FOR = 60 * 60 * 1000;

  // "9:42" for what is left of `ms`.
  function clock(ms) {
    var seconds = Math.max(0, Math.ceil(ms / 1000));
    var s = seconds % 60;
    return Math.floor(seconds / 60) + ':' + (s < 10 ? '0' : '') + s;
  }

  // "482 913": the code in groups of three, kept as one word for copying and selecting.
  function groupedCode(code) {
    var groups = String(code).match(/.{1,3}/g) || [];
    return el('span', { class: 'fam-bigcode' }, groups.map(function (g) { return el('span', null, g); }));
  }

  function doneButton() {
    return el('button', { class: 'btn btn-primary fam-done', type: 'button', onclick: F.closeSheet }, 'Done');
  }

  function footer(pop, left, right) {
    F.clear(pop.foot);
    F.append(pop.foot, [el('div', { class: 'fam-pop-foot-left' }, left), el('div', { class: 'fam-pop-foot-right' }, right)]);
  }

  function subLine(pop, parts, icon) {
    pop.sub.className = 'fam-pop-sub';
    F.clear(pop.sub);
    F.append(pop.sub, [el('span', { class: 'fam-sub-ico', 'aria-hidden': 'true' }, F.icon(icon || 'clock'))].concat(parts));
  }

  // A numbered way to use the link.
  function way(n, title, content) {
    return el('section', { class: 'fam-way' },
      el('h3', { class: 'fam-way-title' }, el('span', { class: 'fam-way-n', 'aria-hidden': 'true' }, String(n)), title),
      content);
  }

  // The QR code on its raised panel, with its step label and caption.
  function qrPanel(link, caption) {
    return el('figure', { class: 'fam-qr-panel' },
      el('h3', { class: 'fam-way-title' }, el('span', { class: 'fam-way-n', 'aria-hidden': 'true' }, '1'), 'Scan'),
      F.qrCard(link),
      el('figcaption', null, caption));
  }

  // A grey stand-in for the QR code, the link and the code card while Octo answers.
  function skeleton(withCode) {
    return el('div', { class: 'fam-pair', 'aria-busy': 'true' },
      el('div', { class: 'fam-qr-panel', 'aria-hidden': 'true' },
        el('span', { class: 'fam-skel fam-skel-label', style: 'width:70px;height:12px' }),
        el('div', { class: 'fam-qr-tile fam-skel' }),
        el('span', { class: 'fam-skel', style: 'width:150px;height:12px' })),
      el('div', { class: 'fam-pair-info', 'aria-hidden': 'true' },
        el('div', { class: 'fam-way' }, el('span', { class: 'fam-skel', style: 'width:180px;height:12px' }), el('div', { class: 'fam-skel fam-skel-row' })),
        withCode ? el('div', { class: 'fam-way' }, el('span', { class: 'fam-skel', style: 'width:220px;height:12px' }), el('div', { class: 'fam-skel fam-skel-card' })) : null),
      el('p', { class: 'visually-hidden' }, 'Loading'));
  }

  function skeletonRows(rows) {
    var table = el('div', { class: 'fam-table fam-skel-table', 'aria-hidden': 'true' });
    for (var i = 0; i < rows; i++) {
      table.appendChild(el('div', { class: 'fam-trow' },
        el('span', { class: 'fam-skel', style: 'width:84px' }),
        el('span', { class: 'fam-skel', style: 'width:' + (i === rows - 1 ? 190 : 150) + 'px' })));
    }
    return el('div', { class: 'fam-pop-stack', 'aria-busy': 'true' }, table, el('p', { class: 'visually-hidden' }, 'Loading'));
  }

  // One plain line and a retry button, with Octo's reason under it when it gave one.
  function errorLine(line, reply, retry) {
    var reason = reply && reply.data && reply.data.error;
    return el('div', { class: 'fam-pop-error', role: 'alert' },
      el('span', { class: 'fam-pop-error-ico' }, F.icon('warning')),
      el('div', { class: 'fam-pop-error-text' },
        el('p', { class: 'fam-pop-error-line' }, line),
        reason ? el('p', { class: 'fam-help' }, reason) : null),
      el('button', { class: 'btn btn-sm', type: 'button', onclick: retry }, F.icon('retry'), 'Retry'));
  }

  function hostOf(url) {
    try { return new URL(url).host; } catch (e) { return url || ''; }
  }

  var whereCount = 0;

  // Where the link or address is for: two equal options, Anywhere and At home, the chosen one
  // filled with a check, and one line under them saying what it uses. Anywhere is hidden for
  // someone who may not listen away; without an outside address it stays choosable, marked
  // with an info icon, and asks for one.
  function linkChoice(answer, choice, onPick, forName) {
    var homeBase = (answer.servers && answer.servers.home) || (answer.links && answer.links.home) || answer.server || '';
    var uses = el('p', { class: 'fam-where-uses' + (choice === 'home' && answer.homeOnly ? ' warn' : '') },
      choice === 'anywhere'
        ? (answer.anywhereAvailable ? 'Uses ' + answer.publicUrl : 'Needs your outside address first')
        : 'Uses your home network (' + hostOf(homeBase) + ')');
    if (answer.awayAllowed === false) {
      return el('div', { class: 'fam-where-wrap' },
        el('p', { class: 'fam-choice-note' }, F.icon('house'),
          el('span', null, 'At home only: ' + (forName || 'this account') + ' does not listen away from home.')),
        uses);
    }
    var id = 'fam-where-' + (++whereCount);
    var options = [['anywhere', 'Anywhere', 'globe'], ['home', 'At home', 'house']];
    var buttons = [];
    var group = el('div', { class: 'fam-where', role: 'radiogroup', 'aria-labelledby': id });
    options.forEach(function (option, i) {
      var on = option[0] === choice;
      var missing = option[0] === 'anywhere' && !answer.anywhereAvailable;
      var button = el('button', {
        type: 'button',
        class: 'fam-where-opt' + (on ? ' on' : ''),
        role: 'radio',
        'aria-checked': on ? 'true' : 'false',
        tabindex: on ? '0' : '-1',
        onclick: function () { if (!on) onPick(option[0]); },
        onkeydown: function (e) {
          var step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
          if (!step) return;
          e.preventDefault();
          var next = options[(i + step + options.length) % options.length][0];
          onPick(next);
          requestAnimationFrame(function () {
            var again = document.querySelector('dialog[open] .fam-where-opt[aria-checked="true"]');
            if (again) again.focus();
          });
        }
      },
      F.icon(option[2]),
      el('span', { class: 'fam-where-text' }, option[1]),
      missing ? el('span', { class: 'fam-where-info', title: 'Needs your outside address' }, F.icon('info'), el('span', { class: 'visually-hidden' }, ', needs your outside address')) : null,
      el('span', { class: 'fam-where-check', 'aria-hidden': 'true' }, F.icon('check')));
      buttons.push(button);
      group.appendChild(button);
    });
    return el('div', { class: 'fam-where-wrap' },
      el('p', { class: 'fam-where-label', id: id }, forName ? 'Where will they use it?' : 'Where will you use it?'),
      group,
      uses);
  }

  // What the "At home only" choice, or a missing outside address, means, above the QR code.
  function homeNote(answer, choice, regenerate) {
    if (choice === 'anywhere' && !answer.anywhereAvailable) {
      return el('section', { class: 'fam-needs-address' },
        el('h3', { class: 'fam-needs-title' }, F.icon('globe'), 'Set your outside address first'),
        el('p', { class: 'fam-help' }, 'Links that work anywhere use the address people reach Octo at from outside, through your reverse proxy or tunnel.'),
        session.owner
          ? el('div', { class: 'fam-stack' },
            el('div', null, el('a', { class: 'btn btn-sm', href: STATUS_ADDRESS, target: '_blank', rel: 'noopener' }, F.icon('out'), 'Set it on the Status page')),
            addressField(function (url, words) { if (url && regenerate) regenerate(words); }))
          : el('p', { class: 'fam-help' }, 'Ask whoever runs this server to set its outside address.'));
    }
    return null;
  }

  function pickDefault(answer) {
    return answer.anywhereAvailable ? 'anywhere' : 'home';
  }

  // Add a device: a pairing code for the Octo app, or an app password for any other app.
  // `member` is null for the signed-in person's own device, or the member a manager adds for.
  //
  // While the code view is open, a code about to run out is swapped for a new one in place,
  // and the new one ends the old (`replaces`), so one code is live per dialog. Swapping waits
  // while the page is hidden, and stops after an hour open.
  function deviceDialog(member) {
    var route = member ? '/members/' + encodeURIComponent(member.username) + '/devices' : '/me/devices';
    var forName = member ? member.displayName : null;
    var pop = F.openDialog({ wide: true, avatar: member ? member.displayName : session.displayName });
    var done = doneButton();
    var code = null;
    var password = null;
    var choice = null;
    var turn = 0;
    var madePassword = false;
    var openedAt = Date.now();
    var ticker = null;
    var retry = null;
    var view = null;
    var flash = null;

    function stop() {
      if (ticker) clearInterval(ticker);
      if (retry) clearTimeout(retry);
      ticker = null;
      retry = null;
      view = null;
    }
    function onVisibility() {
      if (view && !document.hidden) view.tick();
    }
    document.addEventListener('visibilitychange', onVisibility);
    pop.onClose(stop);
    pop.onClose(function () { document.removeEventListener('visibilitychange', onVisibility); });
    pop.onClose(function () {
      if (member) refreshFamily();
      else if (madePassword || code) viewIfShowing('devices');
    });

    function ask(kind) {
      return api('POST', route, { name: '', kind: kind, replaces: code ? code.deviceId || '' : '' });
    }

    function codeFooter() {
      footer(pop,
        el('button', { class: 'link-button fam-foot-link', type: 'button', onclick: function () { showApps(); } }, 'Using Symfonium or another app instead?'),
        done);
    }

    async function showCode(fresh) {
      var mine = ++turn;
      stop();
      pop.title.textContent = member ? 'Add a device for ' + member.displayName : 'Add a device';
      codeFooter();
      if (code && !fresh && Date.parse(code.expires) - Date.now() > REFRESH_LEAD) return drawCode(code);
      if (fresh) openedAt = Date.now();
      subLine(pop, ['Making a code']);
      F.clear(pop.body).appendChild(skeleton(3));
      var reply = await ask('OctoApp');
      if (!pop.isOpen() || mine !== turn) return;
      if (!reply.ok) {
        pop.sub.textContent = 'Works once';
        F.clear(pop.body).appendChild(errorLine('Couldn’t make a code. Try again.', reply, function () { showCode(true); }));
        return;
      }
      code = reply.data;
      if (!choice || (choice === 'anywhere' && code.awayAllowed === false)) choice = pickDefault(code);
      drawCode(code);
    }

    // Past the hour: no more codes until someone asks for one.
    function stillThere() {
      stop();
      turn++;
      subLine(pop, ['Works once · expired']);
      var again = el('button', { class: 'btn btn-primary', type: 'button', onclick: function () { showCode(true); } }, F.icon('retry'), 'Make a new code');
      F.clear(pop.body).appendChild(el('div', { class: 'fam-pop-idle' },
        el('span', { class: 'page-icon', 'aria-hidden': 'true' }, F.icon('clock')),
        el('p', { class: 'fam-pop-idle-title' }, 'Still there? Make a new code'),
        el('p', { class: 'fam-help' }, 'The code stopped renewing after an hour, so codes never pile up.'),
        again));
      pop.announcer.textContent = 'This code expired.';
      if (pop.body.contains(document.activeElement) || document.activeElement === document.body) again.focus();
    }

    function linkOf(added) {
      if (added.links && added.links[choice]) return added.links[choice];
      if (added.links && added.links.home) return added.links.home;
      return F.joinLink(added.server, added.username, added.pairCode);
    }

    function serverOf(added) {
      return (added.servers && added.servers[choice]) || (added.servers && added.servers.home) || added.server;
    }

    // One card for typing it in: the code, large and grouped, with its copy button, and who
    // and where it is for underneath.
    function codeCard(added) {
      return el('div', { class: 'fam-codecard' },
        el('div', { class: 'fam-codecard-top' },
          el('span', { class: 'fam-codecard-label' }, 'Code'),
          el('span', { class: 'fam-codecard-code' }, groupedCode(added.pairCode)),
          F.copyIcon(added.pairCode, 'Copy the code')),
        el('p', { class: 'fam-codecard-for' }, 'for ', el('strong', null, added.username), ' on ', el('span', { class: 'fam-mono' }, serverOf(added))));
    }

    function drawCode(added) {
      var mine = turn;
      var top = el('div', { class: 'fam-pop-top' },
        linkChoice(added, choice, function (v) { choice = v; drawCode(code); }, forName),
        flash ? el('p', { class: 'inline-note ok' }, F.icon('check'), flash) : null);
      flash = null;
      var needs = homeNote(added, choice, function (words) { flash = words; showCode(true); });
      var count = el('span', { class: 'fam-count' });
      var fresh = el('span', { class: 'fam-badge good fam-new', hidden: true }, F.icon('check'), 'New code');
      subLine(pop, ['Works once · ', count, fresh]);
      F.clear(pop.body);
      if (choice === 'anywhere' && !added.anywhereAvailable) {
        F.append(pop.body, [top, needs]);
      } else {
        var note = el('p', { class: 'fam-pop-note', role: 'status', hidden: true });
        var figure = qrPanel(linkOf(added), 'Scan with the new phone’s camera');
        var tile = figure.querySelector('.fam-qr-tile');
        var linkSlot = el('div', { class: 'fam-swap-slot' }, F.linkRow(linkOf(added), 'Copy the link'));
        var codeRow = codeCard(added);
        var pair = el('div', { class: 'fam-pair' },
          figure,
          el('div', { class: 'fam-pair-info' },
            way(2, 'Or open this link on that device', linkSlot),
            way(3, el('span', null, 'Or type it in Octo: ', el('strong', null, 'Join with a family code')), codeRow)));
        F.append(pop.body, [top, needs, pair, note]);
      }
      var freshTimer = null;
      var warned = false;
      var asking = false;
      var stale = false;

      function setStale(on) {
        stale = on;
        if (!pair) return;
        pair.classList.toggle('is-stale', on);
        pair.querySelectorAll('.fam-pair-info button').forEach(function (b) { b.disabled = on; });
        var anchor = linkSlot.querySelector('a');
        if (on && anchor) {
          anchor.removeAttribute('href');
          anchor.setAttribute('aria-disabled', 'true');
        }
      }

      // The new code goes in where the old one was: the QR crossfades, the link and the code
      // fade in, and a "New code" notice shows for 3 seconds.
      function swap(next) {
        code = next;
        if (!pair) return;
        var link = linkOf(next);
        var old = tile.querySelector('svg, p');
        var drawn = F.qrCard(link).firstChild;
        if (old) old.classList.add('fam-leaving');
        drawn.classList.add('fam-arriving');
        tile.appendChild(drawn);
        setTimeout(function () { if (old) old.remove(); }, 400);
        var row = F.linkRow(link, 'Copy the link');
        row.classList.add('fam-arriving');
        F.clear(linkSlot).appendChild(row);
        var nextRow = codeCard(next);
        nextRow.classList.add('fam-arriving');
        codeRow.replaceWith(nextRow);
        codeRow = nextRow;
        setStale(false);
        note.hidden = true;
        warned = false;
        fresh.hidden = false;
        clearTimeout(freshTimer);
        freshTimer = setTimeout(function () { fresh.hidden = true; }, 3000);
        pop.announcer.textContent = 'New code.';
      }

      async function renew() {
        if (asking) return;
        asking = true;
        if (retry) clearTimeout(retry);
        retry = null;
        var reply = await ask('OctoApp');
        asking = false;
        if (!pop.isOpen() || mine !== turn) return;
        if (reply.ok) {
          swap(reply.data);
          tick();
          return;
        }
        if (note) {
          note.textContent = 'Couldn’t get a new code. Trying again.';
          note.hidden = false;
        }
        retry = setTimeout(function () {
          retry = null;
          if (!document.hidden) renew();
        }, REFRESH_RETRY);
      }

      function tick() {
        if (!pop.isOpen() || mine !== turn) return;
        var left = Date.parse(code.expires) - Date.now();
        if (left > 0) {
          count.textContent = 'expires in ' + clock(left);
          count.classList.toggle('warn', left < 60000);
        } else {
          count.textContent = 'expired';
          count.classList.add('warn');
          if (!stale) setStale(true);
        }
        if (!warned && left <= 60000 && left > REFRESH_LEAD) {
          warned = true;
          pop.announcer.textContent = 'One minute left on this code.';
        }
        if (document.hidden || left > REFRESH_LEAD) return;
        if (Date.now() - openedAt >= REFRESH_FOR) {
          if (left <= 0) stillThere();
          return;
        }
        if (!retry) renew();
      }

      view = { tick: tick };
      tick();
      ticker = setInterval(tick, 1000);
    }

    async function showApps() {
      var mine = ++turn;
      stop();
      pop.title.textContent = 'Add Symfonium or another app' + (member ? ' for ' + member.displayName : '');
      footer(pop,
        el('button', { class: 'link-button fam-foot-link', type: 'button', onclick: function () { showCode(false); } }, F.icon('back'), 'Back to the QR code'),
        done);
      if (password) return drawApps(password);
      pop.sub.textContent = 'Making an app password';
      F.clear(pop.body).appendChild(skeletonRows(3));
      // The app password ends the code shown before it, so no code is left live.
      var reply = await ask('SubsonicApp');
      if (!pop.isOpen() || mine !== turn) return;
      if (!reply.ok) {
        pop.sub.textContent = '';
        F.clear(pop.body).appendChild(errorLine('Couldn’t make an app password. Try again.', reply, function () { showApps(); }));
        return;
      }
      password = reply.data;
      code = null;
      madePassword = true;
      if (!choice) choice = pickDefault(password);
      drawApps(password);
    }

    function drawApps(added) {
      subLine(pop, ['Shown once. Copy it now.'], 'key');
      pop.sub.classList.add('warn');
      var bar = linkChoice(added, choice, function (v) { choice = v; drawApps(added); }, forName);
      var server = (added.servers && added.servers[choice]) || null;
      var body = choice === 'anywhere' && !server
        ? homeNote(added, choice, null)
        : F.otherApp(server || added.server, added.username, added.appPassword);
      F.clear(pop.body);
      F.append(pop.body, [el('div', { class: 'fam-pop-top' }, bar), body]);
    }

    showCode(false);
    done.focus();
  }

  // Invite a member, or send a joined member a new sign-in link. `answer` is an invite answer
  // already in hand (just after adding the member); without one the dialog asks for a link.
  function inviteDialog(m, answer) {
    var name = m.displayName;
    var pop = F.openDialog({ wide: true, avatar: name });
    var done = doneButton();
    var turn = 0;
    var choice = null;
    var current = null;
    var flash = null;
    pop.title.textContent = m.joined ? 'A new sign-in link for ' + name : 'Invite ' + name;
    var again = el('button', { class: 'btn btn-ghost', type: 'button', onclick: function () { load(); } }, F.icon('retry'), 'Send a new link');
    footer(pop, again, done);

    function lifetime(days) {
      days = Number(days) || 7;
      return 'The link works once and expires in ' + (days === 1 ? '1 day' : days + ' days');
    }

    function draw(data) {
      current = data;
      if (!choice || (choice === 'anywhere' && data.awayAllowed === false)) choice = pickDefault(data);
      subLine(pop, [lifetime(data.inviteDays)]);
      var link = (data.links && data.links[choice]) || data.inviteLink;
      var top = el('div', { class: 'fam-pop-top' },
        linkChoice(data, choice, function (v) { choice = v; draw(current); }, name),
        flash ? el('p', { class: 'inline-note ok' }, F.icon('check'), flash) : null);
      flash = null;
      var needs = homeNote(data, choice, function (words) { flash = words; load(); });
      F.clear(pop.body);
      if (choice === 'anywhere' && !data.anywhereAvailable) {
        F.append(pop.body, [top, needs]);
        return;
      }
      F.append(pop.body, [top, needs, el('div', { class: 'fam-pair' },
        qrPanel(link, name + ' scans this with their phone'),
        el('div', { class: 'fam-pair-info' },
          way(2, 'Or send ' + name + ' this link',
            F.linkRow(link, 'Copy the link', { title: 'Join my Octo family', text: 'Your sign-in link for Octo:' })),
          way(3, 'Then', el('p', { class: 'fam-pop-line' }, m.joined
            ? name + ' picks a new password. Their apps stay signed in.'
            : name + ' picks a password, then adds their devices.'))))]);
    }

    async function load() {
      var mine = ++turn;
      again.disabled = true;
      pop.sub.textContent = 'Making a link';
      F.clear(pop.body).appendChild(skeleton(0));
      var r = await api('POST', '/members/' + encodeURIComponent(m.username) + '/invite', {});
      if (!pop.isOpen() || mine !== turn) return;
      again.disabled = false;
      if (!r.ok) {
        pop.sub.textContent = '';
        F.clear(pop.body).appendChild(errorLine('Couldn’t make a link. Try again.', r, load));
        return;
      }
      draw(r.data);
    }

    if (answer) draw(answer);
    else load();
    done.focus();
  }

  // ── Routing ──────────────────────────────────────────────────────────────────────────
  var VIEWS = {
    overview: viewOverview,
    devices: viewDevices,
    requests: viewRequests,
    saved: viewSaved,
    library: viewLibrary,
    quality: viewQuality,
    account: viewAccount,
    members: viewMembers,
    inbox: viewInbox,
    'all-devices': viewAllDevices,
    playlists: viewPlaylists,
    activity: viewActivity
  };

  function allowed(view) {
    var nav = navFor(session);
    return nav.you.concat(nav.family).indexOf(view) >= 0;
  }

  async function route() {
    if (!session) return;
    var view = currentView();
    if (!VIEWS[view] || !allowed(view)) view = defaultView(session);
    var main = frame(view);
    document.title = (VIEWS_INFO[view] ? VIEWS_INFO[view][0] + ' · ' : '') + 'Octo Family';
    await VIEWS[view](main);
    var heading = main.querySelector('h1');
    if (heading && (document.activeElement === document.body || !document.activeElement)) heading.focus({ preventScroll: true });
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

  // The dashboard's "Open Family" hands its admin over with a single-use token in the
  // fragment: it is exchanged for the page's own sign-in, then cleared from the address bar.
  async function handoff() {
    var match = /[#&]handoff=([^&]+)/.exec(location.hash);
    if (!match) return;
    history.replaceState(null, '', location.pathname + location.search);
    var reply = await api('POST', '/handoff', { token: decodeURIComponent(match[1]) });
    if (!reply.ok && reply.status !== 401) F.toast(F.errorOf(reply), true);
  }

  // An invite link opened here goes to the join page.
  if (/invite=/.test(location.hash) || /[#&]c=\d/.test(location.hash)) {
    location.replace('/family/join' + location.hash);
    return;
  }
  window.addEventListener('hashchange', function () { F.closeSheet(); closeDrawer(); route(); });
  handoff().then(start);
})();
