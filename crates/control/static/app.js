const $ = (s) => document.querySelector(s);

let token = localStorage.getItem('oswitch_token') || '';
let polling = false;

async function api(path, opts = {}) {
  const res = await fetch('/api' + path, {
    ...opts,
    headers: {
      Authorization: 'Bearer ' + token,
      ...(opts.body ? { 'Content-Type': 'application/json' } : {}),
    },
  });
  if (res.status === 401) {
    showLogin();
    throw new Error('unauthorized');
  }
  return res;
}

function showLogin() {
  $('#login').style.display = 'block';
  $('#boxes').style.display = 'none';
  $('#logout').style.display = 'none';
  $('#token-input').focus();
}

function showApp() {
  $('#login').style.display = 'none';
  $('#boxes').style.display = 'block';
  $('#logout').style.display = 'inline';
}

$('#login-btn').onclick = login;
$('#token-input').addEventListener('keydown', (e) => e.key === 'Enter' && login());
$('#logout').onclick = () => {
  token = '';
  localStorage.removeItem('oswitch_token');
  showLogin();
};

async function login() {
  token = $('#token-input').value.trim();
  const res = await api('/state');
  if (res.ok) {
    localStorage.setItem('oswitch_token', token);
    showApp();
    tick();
  } else {
    $('#token-input').value = '';
  }
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class') node.className = v;
    else if (k.startsWith('on')) node[k] = v;
    else node.setAttribute(k, v);
  }
  for (const c of children) node.append(c);
  return node;
}

async function switchOs(boxId, osId) {
  if (polling) return;
  polling = true; // disable all buttons until the next poll reflects the new phase
  await api(`/boxes/${encodeURIComponent(boxId)}/switch`, {
    method: 'POST',
    body: JSON.stringify({ os: osId }),
  }).catch(() => {});
}

function render(state) {
  const boxes = $('#boxes');
  boxes.innerHTML = '';
  for (const box of state.boxes) {
    const phaseName = box.phase.phase;
    const target = box.phase.target || '';
    const switching = phaseName === 'switching';
    const stuck = phaseName === 'stuck';

    const section = el('div', { class: 'box' });
    section.append(el('div', { class: 'box-title' }, box.name));

    for (const node of box.nodes) {
      const isTarget = target === node.os_id;
      const cls = ['node'];
      if (node.online) cls.push('online');
      if (isTarget && (switching || stuck)) cls.push('switching-target');

      const input = el('input', {
        type: 'radio',
        name: 'box-' + box.id,
        value: node.os_id,
        ...(node.online ? { checked: '' } : {}),
        ...(node.online || switching ? { disabled: '' } : {}),
        onclick: () => switchOs(box.id, node.os_id),
      });
      section.append(el('label', { class: cls.join(' ') }, input,
        el('span', { class: 'label' }, node.os_id),
        el('span', { class: 'dot' })));
    }

    const status = el('div', { class: 'status' });
    if (switching) {
      status.className = 'status switching';
      status.append(box.phase.stage === 'waking'
        ? `waking box, waiting for it to boot…`
        : `switching to ${target}…`);
    } else if (stuck) {
      status.className = 'status stuck';
      status.append('stuck: ' + box.phase.error + ' — press a button to retry');
    }
    section.append(status);
    boxes.append(section);
  }
}

async function tick() {
  const res = await api('/state');
  if (!res.ok) return;
  render(await res.json());
  polling = false;
}

if (token) {
  showApp();
  tick();
} else {
  showLogin();
}
setInterval(tick, 4000);
