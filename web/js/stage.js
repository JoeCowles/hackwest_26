// 3D rack stage: seven Macs on a lit shelf, rendered with three.js.
// three.js is an optional enhancement — loaded async against a 3s budget, never
// retried. The console's other views never wait on it.
import { NODES, POS, statusColor } from './data.js';

const THREE_URL = 'https://cdn.jsdelivr.net/npm/three@0.149.0/build/three.min.js';

export function loadThree(budgetMs = 3000) {
  if (window.THREE) return Promise.resolve(true);
  return new Promise((resolve) => {
    let done = false;
    const finish = (ok) => { if (done) return; done = true; clearTimeout(timer); resolve(ok); };
    const timer = setTimeout(() => finish(false), budgetMs);
    const s = document.createElement('script');
    s.async = true;
    s.src = THREE_URL;
    s.onload = () => finish(!!window.THREE);
    s.onerror = () => finish(false);
    document.head.appendChild(s);
  });
}

/**
 * Boot the stage inside `host`. Returns a controller:
 *   request()      — schedule a frame
 *   setHover(id)   — highlight a host (or null)
 *   isActive()     — set by the caller via opts.isActive
 *   size()         — current [w, h]
 *   dispose()      — tear everything down
 * opts: { blueprintGrid, onHover(id, [x, y]), onOpen(id), isActive() }
 */
export function bootStage(host, opts = {}) {
  const T = window.THREE;
  if (!T || !host) return null;
  const w = host.clientWidth || 900, h = host.clientHeight || 460;

  const renderer = new T.WebGLRenderer({ antialias: false, alpha: true, powerPreference: 'low-power' });
  renderer.setPixelRatio(1);
  renderer.setSize(w, h);
  if ('outputEncoding' in renderer) renderer.outputEncoding = T.sRGBEncoding;
  host.appendChild(renderer.domElement);

  const scene = new T.Scene();
  const camera = new T.PerspectiveCamera(30, w / h, 0.1, 120);
  let vw = w, vh = h;
  const fit = (ww, hh) => {
    if (!ww || !hh) return;
    const asp = ww / hh;
    camera.aspect = asp;
    const half = Math.tan((30 * Math.PI / 180) / 2);
    const dz = Math.max(7.0 / (half * asp), 3.2 / half);
    camera.position.set(0, Math.max(3.4, dz * 0.33), dz);
    camera.lookAt(0, 1.0, 0);
    camera.updateProjectionMatrix();
  };
  fit(w, h);

  // studio environment from a canvas gradient → real glossy reflections
  const c = document.createElement('canvas'); c.width = 16; c.height = 128;
  const g = c.getContext('2d');
  const gr = g.createLinearGradient(0, 0, 0, 128);
  gr.addColorStop(0, '#ffffff'); gr.addColorStop(0.44, '#eceef0');
  gr.addColorStop(0.52, '#c9ccd1'); gr.addColorStop(1, '#9aa0a6');
  g.fillStyle = gr; g.fillRect(0, 0, 16, 128);
  const envTex = new T.CanvasTexture(c);
  envTex.mapping = T.EquirectangularReflectionMapping;
  const pmrem = new T.PMREMGenerator(renderer);
  scene.environment = pmrem.fromEquirectangular(envTex).texture;
  pmrem.dispose();

  scene.add(new T.HemisphereLight(0xffffff, 0xd7d8da, 0.5));
  const key = new T.DirectionalLight(0xffffff, 1.15);
  key.position.set(5.5, 9, 6.5);
  scene.add(key);
  const fill = new T.DirectionalLight(0xffffff, 0.35);
  fill.position.set(-7, 4, -4); scene.add(fill);

  // contact shadows are painted, not shadow-mapped — same look, no per-frame cost
  const shc = document.createElement('canvas'); shc.width = shc.height = 128;
  const sg = shc.getContext('2d');
  const rg = sg.createRadialGradient(64, 64, 4, 64, 64, 62);
  rg.addColorStop(0, 'rgba(29,31,32,.42)'); rg.addColorStop(0.55, 'rgba(29,31,32,.16)'); rg.addColorStop(1, 'rgba(29,31,32,0)');
  sg.fillStyle = rg; sg.fillRect(0, 0, 128, 128);
  const shadowMat = new T.MeshBasicMaterial({ map: new T.CanvasTexture(shc), transparent: true, depthWrite: false });

  const ground = new T.Mesh(new T.PlaneGeometry(60, 60), new T.MeshStandardMaterial({ color: 0xe8e9ea, roughness: 0.92, metalness: 0 }));
  ground.rotation.x = -Math.PI / 2; ground.position.y = -0.02;
  scene.add(ground);

  if (opts.blueprintGrid !== false) {
    const grid = new T.GridHelper(60, 60, 0x5980a6, 0xc9cacc);
    grid.material.opacity = 0.32; grid.material.transparent = true;
    grid.position.y = 0.001; scene.add(grid);
  }

  const alu = new T.MeshStandardMaterial({ color: 0xd6d7d9, metalness: 0.92, roughness: 0.26 });
  const aluDark = new T.MeshStandardMaterial({ color: 0xb9babc, metalness: 0.9, roughness: 0.34 });
  const glass = new T.MeshStandardMaterial({ color: 0x12141a, metalness: 0.55, roughness: 0.07 });
  const bezel = new T.MeshStandardMaterial({ color: 0x0a0b0d, metalness: 0.3, roughness: 0.5 });

  const rig = new T.Group(); scene.add(rig);
  const hits = [];

  NODES.forEach(n => {
    const grp = new T.Group();
    const [x, z] = POS[n.id];
    grp.position.set(x, 0, z);
    const col = new T.Color(statusColor(n.state));

    // lit base plate — status as light, not decoration
    const plate = new T.Mesh(new T.BoxGeometry(n.kind === 'imac' ? 2.9 : 2.6, 0.035, n.kind === 'imac' ? 1.5 : 1.9),
      new T.MeshStandardMaterial({ color: col, emissive: col, emissiveIntensity: n.state === 'offline' ? 0.15 : 0.55, roughness: 0.4, metalness: 0 }));
    plate.position.y = 0.018; grp.add(plate);

    const sh = new T.Mesh(new T.PlaneGeometry(n.kind === 'imac' ? 4.4 : 4.0, n.kind === 'imac' ? 3.0 : 3.4), shadowMat);
    sh.rotation.x = -Math.PI / 2; sh.position.set(0, 0.006, n.kind === 'imac' ? 0.1 : 0.3);
    grp.add(sh);

    if (n.kind === 'imac') {
      const foot = new T.Mesh(new T.BoxGeometry(1.5, 0.07, 0.85), aluDark);
      foot.position.y = 0.075; grp.add(foot);
      const neck = new T.Mesh(new T.BoxGeometry(0.44, 0.75, 0.1), aluDark);
      neck.position.set(0, 0.45, -0.05); grp.add(neck);
      const back = new T.Mesh(new T.BoxGeometry(2.55, 1.52, 0.14), alu);
      back.position.set(0, 1.55, -0.06); grp.add(back);
      const front = new T.Mesh(new T.BoxGeometry(2.48, 1.45, 0.03), bezel);
      front.position.set(0, 1.55, 0.02); grp.add(front);
      const scr = new T.Mesh(new T.PlaneGeometry(2.3, 1.16), glass);
      scr.position.set(0, 1.62, 0.04); grp.add(scr);
      const chin = new T.Mesh(new T.PlaneGeometry(2.3, 0.16), alu);
      chin.position.set(0, 0.9, 0.04); grp.add(chin);
    } else {
      const base = new T.Mesh(new T.BoxGeometry(2.2, 0.075, 1.5), alu);
      base.position.set(0, 0.075, 0.35); grp.add(base);
      const pad = new T.Mesh(new T.PlaneGeometry(1.9, 1.15), new T.MeshStandardMaterial({ color: 0xc2c3c5, metalness: 0.7, roughness: 0.45 }));
      pad.rotation.x = -Math.PI / 2; pad.position.set(0, 0.114, 0.35); grp.add(pad);
      const hinge = new T.Group(); hinge.position.set(0, 0.09, -0.4); grp.add(hinge);
      const lid = new T.Mesh(new T.BoxGeometry(2.2, 1.42, 0.06), alu);
      lid.position.set(0, 0.71, -0.02); hinge.add(lid);
      const lscr = new T.Mesh(new T.PlaneGeometry(2.02, 1.24), glass);
      lscr.position.set(0, 0.71, 0.02); hinge.add(lscr);
      hinge.rotation.x = n.state === 'offline' ? -1.34 : -0.28;
    }

    const hit = new T.Mesh(new T.BoxGeometry(2.7, 2.4, 2.0), new T.MeshBasicMaterial({ visible: false }));
    hit.position.y = 1.2; hit.userData.id = n.id; grp.add(hit); hits.push(hit);
    grp.userData.plate = plate;
    grp.userData.id = n.id;
    grp.userData.node = n;
    grp.userData.labelY = n.kind === 'imac' ? 2.5 : 2.15;
    rig.add(grp);
  });

  // 3D-locked HTML labels
  const layer = document.createElement('div');
  layer.style.cssText = 'position:absolute;inset:0;pointer-events:none';
  host.appendChild(layer);
  const labels = {};
  NODES.forEach(n => {
    const d = document.createElement('div');
    d.className = 'stage-label';
    d.style.borderColor = statusColor(n.state);
    d.style.color = n.state === 'healthy' ? '#2c455d' : statusColor(n.state);
    d.textContent = n.name;
    layer.appendChild(d); labels[n.id] = d;
  });

  const ray = new T.Raycaster(), ptr = new T.Vector2();
  let drag = null, spin = 0, target = 0, hoverId = null, raf = null, disposed = false;
  const el = renderer.domElement;

  const request = () => { if (!raf && !disposed) raf = requestAnimationFrame(frame); };
  const active = () => !document.hidden && host.clientWidth && (!opts.isActive || opts.isActive());

  const onMove = (e) => {
    const r = el.getBoundingClientRect();
    if (drag !== null) { target = drag.spin + (e.clientX - drag.x) * 0.006; request(); return; }
    ptr.x = ((e.clientX - r.left) / r.width) * 2 - 1;
    ptr.y = -((e.clientY - r.top) / r.height) * 2 + 1;
    ray.setFromCamera(ptr, camera);
    const hit = ray.intersectObjects(hits, false)[0];
    const id = hit ? hit.object.userData.id : null;
    const xy = [e.clientX - r.left, e.clientY - r.top];
    if (id !== hoverId) { hoverId = id; request(); }
    if (opts.onHover) opts.onHover(id, xy);
    el.style.cursor = id ? 'pointer' : 'grab';
  };
  const onLeave = () => { hoverId = null; if (opts.onHover) opts.onHover(null, [0, 0]); request(); };
  const onDown = (e) => { drag = { x: e.clientX, spin: target }; el.style.cursor = 'grabbing'; };
  const onUp = () => { drag = null; if (el.style.cursor === 'grabbing') el.style.cursor = hoverId ? 'pointer' : 'grab'; };
  const onClick = () => { if (hoverId && opts.onOpen) opts.onOpen(hoverId); };
  el.addEventListener('pointermove', onMove);
  el.addEventListener('pointerleave', onLeave);
  el.addEventListener('pointerdown', onDown);
  el.addEventListener('click', onClick);
  window.addEventListener('pointerup', onUp);

  const v = new T.Vector3();
  function frame() {
    raf = null;
    if (disposed || !active()) return;
    const tt = performance.now() / 1000;
    spin += (target - spin) * 0.14;
    rig.rotation.y = spin;
    let settling = Math.abs(target - spin) > 0.001;
    rig.children.forEach(grp => {
      const n = grp.userData.node;
      const hov = hoverId === grp.userData.id;
      const base = n.state === 'offline' ? 0.15 : 0.55;
      const p = grp.userData.plate;
      p.material.emissiveIntensity += ((hov ? 1.5 : base + (n.state === 'degraded' ? Math.sin(tt * 2.2) * 0.25 : 0)) - p.material.emissiveIntensity) * 0.3;
      grp.position.y += ((hov ? 0.09 : 0) - grp.position.y) * 0.3;
      if (Math.abs(grp.position.y - (hov ? 0.09 : 0)) > 0.002) settling = true;
      const l = labels[grp.userData.id];
      v.set(grp.position.x, grp.userData.labelY, grp.position.z).applyMatrix4(rig.matrixWorld).project(camera);
      const lx = (v.x * 0.5 + 0.5) * vw, ly = (-v.y * 0.5 + 0.5) * vh;
      if (!isFinite(lx) || !isFinite(ly)) return;
      l.style.left = Math.max(26, Math.min(vw - 26, lx)) + 'px';
      l.style.top = Math.max(46, Math.min(vh - 52, ly)) + 'px';
      l.style.opacity = hov ? '1' : '.78';
    });
    renderer.render(scene, camera);
    if (settling) request();
  }
  request();
  // 4Hz heartbeat only for the degraded node's pulsing plate
  const pulse = setInterval(() => { if (active()) request(); }, 250);

  const ro = new ResizeObserver(() => {
    const nw = host.clientWidth, nh = host.clientHeight;
    if (!nw || !nh) return;
    vw = nw; vh = nh;
    fit(nw, nh); renderer.setSize(nw, nh); request();
  });
  ro.observe(host);

  return {
    request,
    setHover(id) { if (id !== hoverId) { hoverId = id; request(); } },
    size() { return [vw, vh]; },
    dispose() {
      disposed = true;
      clearInterval(pulse);
      if (raf) cancelAnimationFrame(raf);
      ro.disconnect();
      el.removeEventListener('pointermove', onMove);
      el.removeEventListener('pointerleave', onLeave);
      el.removeEventListener('pointerdown', onDown);
      el.removeEventListener('click', onClick);
      window.removeEventListener('pointerup', onUp);
      scene.traverse(o => { if (o.geometry) o.geometry.dispose(); if (o.material && o.material.dispose) o.material.dispose(); });
      renderer.dispose();
      if (el.parentNode) el.parentNode.removeChild(el);
      if (layer.parentNode) layer.parentNode.removeChild(layer);
    }
  };
}
