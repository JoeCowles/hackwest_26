// Optional 3D rack for one supplied host page, rendered with vendored Three.js.
// three.js is an optional enhancement — loaded async against a 3s budget, never
// retried. The console's other views never wait on it.
import { statusColor } from './data.js';
import { placeRackLabels } from './rack.js';

const THREE_URL = new URL('./vendor-three.js', import.meta.url).href;

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

// Pointer identity and movement distinguish selection from orbiting. A tap uses
// fresh hit testing; touch devices do not have a preceding hover event.
export function rackPointerHandlers({ pick, onHover, onOpen, onRotate }) {
  let press = null;
  const hover = event => { const id = pick(event); onHover?.(id, event); return id; };
  return {
    down(event) {
      if (press || (event.button != null && event.button !== 0)) return;
      press = { id: event.pointerId, x: event.clientX, y: event.clientY, lastX: event.clientX, moved: false };
      hover(event);
    },
    move(event) {
      if (!press) { hover(event); return; }
      if (press.id !== event.pointerId) return;
      if (Math.hypot(event.clientX - press.x, event.clientY - press.y) > 5) press.moved = true;
      if (press.moved) onRotate?.(event.clientX - press.lastX);
      press.lastX = event.clientX;
    },
    up(event) {
      if (!press || press.id !== event.pointerId) return;
      const tap = !press.moved && Math.hypot(event.clientX - press.x, event.clientY - press.y) <= 5;
      press = null;
      const id = hover(event);
      if (tap && id) onOpen?.(id);
    },
    cancel(event) { if (!event || event.pointerId === press?.id) press = null; onHover?.(null, event); },
    leave(event) { if (!press) onHover?.(null, event); }
  };
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
  const nodes = opts.nodes || [];
  const columns = Math.min(4, nodes.length || 1);
  const rows = Math.max(1, Math.ceil(nodes.length / columns));
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
    const dz = Math.max((columns * 1.7 + 1) / (half * asp), (rows * 1.7 + 1) / half);
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
  const environment = pmrem.fromEquirectangular(envTex);
  scene.environment = environment.texture;
  envTex.dispose();
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

  function makeIMac(grp) {
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
  }
  function makeMacbook(grp,n) {
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
  function makeMacMini(grp) {
    const body=new T.Mesh(new T.BoxGeometry(1.7,0.48,1.7),alu);
    body.position.set(0,0.31,0.1);grp.add(body);
    const base=new T.Mesh(new T.BoxGeometry(1.45,0.06,1.45),bezel);
    base.position.set(0,0.07,0.1);grp.add(base);
    const port=new T.Mesh(new T.BoxGeometry(0.45,0.055,0.015),bezel);
    port.position.set(0,0.27,0.96);grp.add(port);
  }
  function makeUnknown(grp) {
    const body=new T.Mesh(new T.BoxGeometry(1.2,0.9,1.1),aluDark);
    body.position.set(0,0.52,0.15);grp.add(body);
    const face=new T.Mesh(new T.BoxGeometry(1.0,0.68,0.02),bezel);
    face.position.set(0,0.52,0.71);grp.add(face);
    const indicator=new T.Mesh(new T.BoxGeometry(0.5,0.065,0.025),alu);
    indicator.position.set(0,0.5,0.735);grp.add(indicator);
  }

  nodes.forEach((n, index) => {
    const grp = new T.Group();
    const x = ((index % columns) - (columns - 1) / 2) * 3.5;
    const z = (Math.floor(index / columns) - (rows - 1) / 2) * 3.5;
    grp.position.set(x, 0, z);
    const col = new T.Color(statusColor(n.state));

    // lit base plate — status as light, not decoration
    const plate = new T.Mesh(new T.BoxGeometry(n.kind === 'imac' ? 2.9 : 2.6, 0.035, n.kind === 'imac' ? 1.5 : 1.9),
      new T.MeshStandardMaterial({ color: col, emissive: col, emissiveIntensity: n.state === 'offline' ? 0.15 : 0.55, roughness: 0.4, metalness: 0 }));
    plate.position.y = 0.018; grp.add(plate);

    const sh = new T.Mesh(new T.PlaneGeometry(n.kind === 'imac' ? 4.4 : 4.0, n.kind === 'imac' ? 3.0 : 3.4), shadowMat);
    sh.rotation.x = -Math.PI / 2; sh.position.set(0, 0.006, n.kind === 'imac' ? 0.1 : 0.3);
    grp.add(sh);

    ({macbook:makeMacbook,mac_mini:makeMacMini,imac:makeIMac,unknown:makeUnknown}[n.kind] || makeUnknown)(grp,n);

    const hit = new T.Mesh(new T.BoxGeometry(2.7, 2.4, 2.0), new T.MeshBasicMaterial({ visible: false }));
    hit.position.y = 1.2; hit.userData.id = n.id; grp.add(hit); hits.push(hit);
    grp.userData.plate = plate;
    grp.userData.id = n.id;
    grp.userData.node = n;
    grp.userData.labelY = n.kind === 'imac' ? 2.5 : n.kind === 'macbook' ? 2.15 : 1.5;
    rig.add(grp);
  });

  // 3D-locked HTML labels
  const layer = document.createElement('div');
  layer.style.cssText = 'position:absolute;inset:0;pointer-events:none';
  host.appendChild(layer);
  const labels = {};
  nodes.forEach(n => {
    const d = document.createElement('div');
    d.className = 'stage-label';
    d.style.borderColor = statusColor(n.state);
    d.style.color = n.state === 'healthy' ? '#2c455d' : statusColor(n.state);
    d.style.visibility = 'hidden';
    d.textContent = `${n.name} · ${n.kind === 'mac_mini' ? 'Mac mini' : n.kind === 'macbook' ? 'MacBook' : n.kind === 'imac' ? 'iMac' : 'Unknown model'}`;
    layer.appendChild(d); labels[n.id] = d;
  });

  const ray = new T.Raycaster(), ptr = new T.Vector2();
  let spin = 0, target = 0, hoverId = null, raf = null, disposed = false;
  const el = renderer.domElement;

  const request = () => { if (!raf && !disposed) raf = requestAnimationFrame(frame); };
  const active = () => !document.hidden && host.clientWidth && (!opts.isActive || opts.isActive());

  const pick = e => {
    const r = el.getBoundingClientRect();
    ptr.x = ((e.clientX - r.left) / r.width) * 2 - 1;
    ptr.y = -((e.clientY - r.top) / r.height) * 2 + 1;
    ray.setFromCamera(ptr, camera);
    return ray.intersectObjects(hits, false)[0]?.object.userData.id || null;
  };
  const handlers = rackPointerHandlers({ pick,
    onHover: (id, e) => {
      if (id !== hoverId) { hoverId = id; request(); }
      const r = el.getBoundingClientRect();
      opts.onHover?.(id, e ? [e.clientX - r.left, e.clientY - r.top] : [0, 0]);
      el.style.cursor = id ? 'pointer' : 'grab';
    },
    onOpen: opts.onOpen,
    onRotate: delta => { target += delta * 0.006; el.style.cursor = 'grabbing'; request(); }
  });
  const onDown = e => { if (e.button !== 0) return; el.setPointerCapture?.(e.pointerId); handlers.down(e); };
  const onUp = e => { if (el.hasPointerCapture?.(e.pointerId)) el.releasePointerCapture(e.pointerId); handlers.up(e); };
  el.style.touchAction = 'none';
  el.addEventListener('pointermove', handlers.move);
  el.addEventListener('pointerleave', handlers.leave);
  el.addEventListener('pointerdown', onDown);
  el.addEventListener('pointerup', onUp);
  el.addEventListener('pointercancel', handlers.cancel);

  const v = new T.Vector3();
  function frame() {
    raf = null;
    if (disposed || !active()) return;
    const tt = performance.now() / 1000;
    spin += (target - spin) * 0.14;
    rig.rotation.y = spin;
    let settling = Math.abs(target - spin) > 0.001;
    const projected=[];
    rig.children.forEach(grp => {
      const n = grp.userData.node;
      const hov = hoverId === grp.userData.id;
      const base = n.state === 'offline' ? 0.15 : 0.55;
      const p = grp.userData.plate;
      p.material.emissiveIntensity += ((hov ? 1.5 : base + (n.state === 'degraded' ? Math.sin(tt * 2.2) * 0.25 : 0)) - p.material.emissiveIntensity) * 0.3;
      grp.position.y += ((hov ? 0.09 : 0) - grp.position.y) * 0.3;
      if (Math.abs(grp.position.y - (hov ? 0.09 : 0)) > 0.002) settling = true;
      const l = labels[grp.userData.id];
      l.classList.toggle('stage-label-hovered',hov);
      l.style.maxWidth = Math.max(0,Math.min(hov?320:120,vw-16)) + 'px';
      v.set(grp.position.x, grp.userData.labelY, grp.position.z).applyMatrix4(rig.matrixWorld).project(camera);
      const lx = (v.x * 0.5 + 0.5) * vw, ly = (-v.y * 0.5 + 0.5) * vh;
      projected.push({id:grp.userData.id,x:lx,y:ly,depth:v.z});
    });
    // Batch style changes, then measure once before writing final positions.
    // Hidden labels still have dimensions and can take priority on mesh hover.
    const placed=new Map(placeRackLabels(projected.map(p=>({...p,width:labels[p.id].offsetWidth,height:labels[p.id].offsetHeight})),{width:vw,height:vh},hoverId).map(p=>[p.id,p]));
    Object.entries(labels).forEach(([id,l])=>{
      const rect=placed.get(id);
      l.style.visibility=rect?'visible':'hidden';
      if(rect){l.style.left=rect.left+'px';l.style.top=rect.top+'px';}
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
      el.removeEventListener('pointermove', handlers.move);
      el.removeEventListener('pointerleave', handlers.leave);
      el.removeEventListener('pointerdown', onDown);
      el.removeEventListener('pointerup', onUp);
      el.removeEventListener('pointercancel', handlers.cancel);
      const geometries=new Set(),materials=new Set([alu,aluDark,glass,bezel,shadowMat]);
      scene.traverse(o => { if(o.geometry)geometries.add(o.geometry); if(o.material)(Array.isArray(o.material)?o.material:[o.material]).forEach(m=>materials.add(m)); });
      geometries.forEach(g=>g.dispose());materials.forEach(m=>m.dispose());
      environment.dispose();
      shadowMat.map.dispose();
      renderer.dispose();
      if (el.parentNode) el.parentNode.removeChild(el);
      if (layer.parentNode) layer.parentNode.removeChild(layer);
    }
  };
}
