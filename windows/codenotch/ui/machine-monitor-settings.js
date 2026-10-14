(function (root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.MachineMonitorSettings = api;
})(typeof window === 'object' ? window : globalThis, function () {
  'use strict';
  const unique = ids => [...new Set((ids || []).filter(id => typeof id === 'string' && id))];
  function copy(settings) {
    return { enabled: !!settings.enabled, refresh_seconds: settings.refresh_seconds,
      history_minutes: settings.history_minutes, order: unique(settings.order), hidden: unique(settings.hidden) };
  }
  function orderedIds(settings, resources) {
    return unique([...settings.order, ...resources.map(r => r.id)]);
  }
  function createController(invoke, changed = () => {}) {
    let persisted = null, draft = null, resources = [], supported = null, inventorySignature = '';
    let busy = false, loading = true, dirty = false, error = '', saved = false, revision = 0;
    const state = () => ({ persisted: persisted && copy(persisted), draft: draft && copy(draft),
      resources: resources.map(r => ({ ...r })), supported, busy, loading, dirty, error, saved });
    const notify = () => changed(state());
    function acceptSettings(settings) {
      if (!settings || dirty || busy) return;
      revision++;
      persisted = copy(settings); draft = copy(settings); loading = false; error = ''; saved = false; notify();
    }
    function acceptSnapshot(snapshot) {
      if (!snapshot) return;
      const available = snapshot.supported === true;
      const inventory = Array.isArray(snapshot.resources) ? snapshot.resources.filter(r => r && typeof r.id === 'string')
        .map(({ id, kind, name, subtitle, vendor }) => ({ id, kind, name, subtitle, vendor })) : [];
      const signature = JSON.stringify([available, inventory]);
      if (signature === inventorySignature) return;
      inventorySignature = signature; supported = available; resources = inventory;
      notify();
    }
    async function reload() {
      if (busy) return;
      loading = true; error = ''; saved = false; notify();
      const request = ++revision;
      try {
        const settings = await invoke('get_machine_monitor_settings');
        if (request !== revision) return;
        persisted = copy(settings); draft = copy(settings); dirty = false;
      } catch (e) { if (request === revision) error = String(e); }
      finally { if (request === revision) { loading = false; notify(); } }
    }
    function edit(patch) {
      if (!draft || busy || loading || supported !== true) return;
      draft = copy({ ...draft, ...patch }); dirty = JSON.stringify(draft) !== JSON.stringify(persisted);
      error = ''; saved = false; notify();
    }
    function toggle(id, visible) {
      if (!draft) return;
      edit({ hidden: visible ? draft.hidden.filter(x => x !== id) : unique([...draft.hidden, id]) });
    }
    function move(id, delta) {
      if (!draft) return;
      const order = orderedIds(draft, resources);
      const visibleInventory = order.filter(x => resources.some(r => r.id === x));
      const at = visibleInventory.indexOf(id), next = visibleInventory[at + delta];
      if (!next) return;
      const a = order.indexOf(id), b = order.indexOf(next);
      [order[a], order[b]] = [order[b], order[a]];
      edit({ order });
    }
    async function save() {
      if (!draft || !dirty || busy || loading || supported !== true) return false;
      // Keep disconnected devices: inventory is not the authority for saved identity lists.
      const payload = copy({ ...draft, order: orderedIds(draft, resources) });
      busy = true; error = ''; saved = false; revision++; notify();
      try {
        const result = await invoke('set_machine_monitor_settings', { settings: payload });
        persisted = copy(result); draft = copy(result); dirty = false; saved = true; return true;
      } catch (e) { error = String(e); return false; }
      finally { busy = false; notify(); }
    }
    function cancel() {
      if (!persisted || busy) return;
      revision++; draft = copy(persisted); dirty = false; error = ''; saved = false; loading = false; notify();
    }
    return { state, reload, edit, toggle, move, save, cancel, acceptSettings, acceptSnapshot };
  }
  function mount({ invoke, listen, host, tr = x => x }) {
    const doc = host.ownerDocument;
    function el(tag, className, text) {
      const node = doc.createElement(tag); if (className) node.className = className;
      if (text) node.textContent = text; return node;
    }
    const heading = el('h2', 'sec'); const group = el('div', 'group');
    const controls = [];
    function optionRow(key, control) {
      const row = el('label', 'item'); const label = el('span');
      row.append(label, control); group.append(row); controls.push(control); return { key, label };
    }
    const enabled = el('input'); enabled.type = 'checkbox';
    const refresh = el('select'), history = el('select');
    for (const value of [3, 5, 10]) { const o = el('option', '', value + ' s'); o.value = value; refresh.append(o); }
    for (const value of [1, 5, 15]) { const o = el('option', '', value + ' min'); o.value = value; history.append(o); }
    const labels = [optionRow('Show machine monitor', enabled), optionRow('Refresh interval', refresh), optionRow('Graph history', history)];
    const note = el('p', 'item cap'); group.append(note);
    const devicesHeading = el('h2', 'sec'), inventory = el('div', 'group');
    const empty = el('p', 'item cap'), preserved = el('p', 'item cap');
    const actions = el('div', 'item'), save = el('button', 'btn'), cancel = el('button', 'btn'), reload = el('button', 'btn');
    const status = el('p', 'item cap'); status.setAttribute('role', 'status'); status.setAttribute('aria-live', 'polite');
    const error = el('p', 'item cap'); error.setAttribute('role', 'alert'); error.style.color = 'var(--danger)';
    actions.append(save, cancel, reload); host.append(heading, group, devicesHeading, inventory, empty, preserved, actions, status, error);
    let rows = new Map(), lastState;
    function render(state) {
      lastState = state;
      heading.textContent = tr('Machine'); devicesHeading.textContent = tr('Visible resources and order');
      labels.forEach(({ key, label }) => { label.textContent = tr(key); });
      note.textContent = tr('Only selection, order and options are saved; graph history stays in memory.');
      save.textContent = tr('Save'); cancel.textContent = tr('Cancel changes'); reload.textContent = tr('Reload');
      const locked = state.loading || !state.draft || state.busy || state.supported !== true;
      controls.forEach(c => { c.disabled = locked; });
      if (state.draft) { enabled.checked = state.draft.enabled; refresh.value = state.draft.refresh_seconds; history.value = state.draft.history_minutes; }
      save.disabled = locked || !state.dirty; cancel.disabled = state.busy || !state.dirty;
      reload.disabled = state.busy || state.loading;
      status.textContent = state.loading ? tr('Loading…') : state.supported === false ? tr('Machine monitoring is available only on Windows.')
        : state.busy ? tr('Saving…') : state.dirty ? tr('Unsaved changes') : state.saved ? tr('Saved')
        : state.draft && !state.draft.enabled ? tr('Monitoring paused. Cached device inventory is shown.') : '';
      error.textContent = state.error ? tr('Could not load or save monitor settings:') + ' ' + state.error : ''; error.hidden = !state.error;
      empty.hidden = state.resources.length > 0;
      empty.textContent = tr('Waiting for device inventory…');
      const ids = state.draft ? orderedIds(state.draft, state.resources).filter(id => state.resources.some(r => r.id === id)) : state.resources.map(r => r.id);
      const active = new Set(ids);
      for (const [id, row] of rows) if (!active.has(id)) { row.node.remove(); rows.delete(id); }
      ids.forEach((id, index) => {
        let row = rows.get(id);
        if (!row) {
          const node = el('div', 'item'), label = el('label'), checkbox = el('input'), name = el('span'), detail = el('div', 'hint');
          checkbox.type = 'checkbox'; label.append(checkbox, name, detail); label.style.minWidth = '0'; label.style.overflowWrap = 'anywhere';
          const tools = el('div', 'acct-tools'), up = el('button', '', '↑'), down = el('button', '', '↓');
          up.type = down.type = 'button'; tools.append(up, down); node.append(label, tools);
          checkbox.addEventListener('change', () => controller.toggle(id, checkbox.checked));
          up.addEventListener('click', () => controller.move(id, -1)); down.addEventListener('click', () => controller.move(id, 1));
          row = { node, checkbox, name, detail, up, down }; rows.set(id, row);
        }
        const resource = state.resources.find(r => r.id === id);
        row.name.textContent = ' ' + resource.name;
        row.detail.textContent = [resource.kind === 'gpu' ? resource.name : '', resource.subtitle,
          resource.vendor && !String(resource.subtitle || '').includes(resource.vendor) ? resource.vendor : ''].filter(Boolean).join(' · ');
        row.checkbox.checked = !!state.draft && !state.draft.hidden.includes(id); row.checkbox.disabled = locked;
        row.up.disabled = locked || index === 0; row.down.disabled = locked || index === ids.length - 1;
        row.up.setAttribute('aria-label', tr('Move up') + ': ' + resource.name);
        row.down.setAttribute('aria-label', tr('Move down') + ': ' + resource.name);
        // Ordinary metric samples must not detach controls or disturb keyboard focus.
        if (inventory.children[index] !== row.node) inventory.insertBefore(row.node, inventory.children[index] || null);
      });
      const missing = state.draft ? unique([...state.draft.order, ...state.draft.hidden]).filter(id => !active.has(id)).length : 0;
      preserved.hidden = !missing; preserved.textContent = tr('Preferences for disconnected devices are retained.') + (missing ? ' (' + missing + ')' : '');
    }
    const controller = createController(invoke, render);
    enabled.addEventListener('change', () => controller.edit({ enabled: enabled.checked }));
    refresh.addEventListener('change', () => controller.edit({ refresh_seconds: Number(refresh.value) }));
    history.addEventListener('change', () => controller.edit({ history_minutes: Number(history.value) }));
    save.addEventListener('click', () => controller.save()); cancel.addEventListener('click', () => controller.cancel());
    reload.addEventListener('click', () => { controller.reload(); invoke('get_machine_snapshot').then(controller.acceptSnapshot).catch(() => {}); });
    render(controller.state());
    let disposed = false, unlisteners = [];
    const ready = Promise.all(['machine_monitor_settings', 'machine_snapshot'].map(async name => {
      try { const stop = await listen(name, e => { if (!disposed) (name === 'machine_snapshot' ? controller.acceptSnapshot : controller.acceptSettings)(e.payload); });
        if (disposed) stop(); else unlisteners.push(stop);
      } catch (e) { /* Commands still provide a complete initial state. */ }
    })).then(() => Promise.all([controller.reload(), invoke('get_machine_snapshot').then(controller.acceptSnapshot).catch(() => {})]));
    return { controller, ready, render: () => render(lastState), dispose: () => { disposed = true; unlisteners.forEach(stop => stop()); } };
  }
  return { copy, orderedIds, createController, mount };
});
