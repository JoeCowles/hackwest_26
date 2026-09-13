// Bounded live native-storage integration. Builds are explicit, TLS is verified,
// and all processes/state belong to the disposable smoke-ciderd harness.
import assert from 'node:assert/strict';
import path from 'node:path';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { smokeCiderd } from './smoke-ciderd.mjs';

export async function verifyEmptyStorage({ request }) {
  const result = await request('GET', '/api/v1/cluster');
  assert.equal(result.status, 200);
  const cluster = result.json().data;
  assert.equal(cluster.node_counts.total, 0);
  assert.equal(cluster.observed_node_count, 0);
  for (const measurement of [cluster.capacity.local.capacity_bytes,
    cluster.throughput.read_bytes_per_second, cluster.throughput.write_bytes_per_second]) {
    assert.equal(measurement.state, 'unknown');
    assert.equal(measurement.value, null);
  }
}

export async function verifyStorage({ node, viewer, request, run }) {
    let disks, inventory, hardware, detail;
    for (let attempt = 0; attempt < 12; attempt++) {
      detail = await request('GET', `/api/v1/nodes/${node}`, undefined, viewer);
      assert.equal(detail.status, 200);
      hardware = detail.json().data.hardware;
      assert.ok(hardware, 'Native node exposes hardware metadata');
      const result = await request('GET', `/api/v1/nodes/${node}/disks?limit=500`, undefined, viewer);
      assert.equal(result.status, 200, 'Physical disk route is implemented');
      disks = result.json();
      const objects = await request('GET', `/api/v1/nodes/${node}/inventory?limit=500`, undefined, viewer);
      assert.equal(objects.status, 200);
      inventory = objects.json();
      if (disks.meta.topology_revision === inventory.meta.topology_revision &&
          disks.data.some(disk => disk.io?.linkage_state === 'resolved')) break;
      await delay(5000);
    }
    assert.ok(hardware.model_identifier, 'Native hw.model is present');
    assert.equal(hardware.model_identifier, (await run('/usr/sbin/sysctl', ['-n', 'hw.model'])).trim(), 'API reports this Mac\'s actual identifier');
    assert.equal(hardware.source, 'sysctl.hw.model');
    assert.ok(['ok', 'unknown'].includes(hardware.state), 'Current hardware query succeeded');
    if (hardware.state === 'unknown') {
      assert.equal(hardware.reason, 'identifier_unmapped');
      assert.equal(hardware.machine_family, 'unknown');
    } else assert.ok(['macbook', 'mac_mini', 'imac'].includes(hardware.machine_family));
    assert.ok(disks.data.length > 0, 'Native physically backed disks are present');
    assert.equal(disks.meta.node_id, node);
    assert.equal(disks.meta.topology_revision, inventory.meta.topology_revision, 'Summaries and graph have a coherent structural stamp');
    assert.ok(inventory.data.every(object => Array.isArray(object.physical_disk_ids)));
    assert.equal(new Set(disks.data.map(d => d.object_id)).size, disks.data.length, 'Physical disk identities are unique');
    const resolved = disks.data.filter(disk => disk.io.linkage_state === 'resolved');
    assert.ok(resolved.length > 0, 'Native IOKit evidence resolves a physical disk');
    for (const disk of resolved) {
      assert.equal(disk.node_id, node);
      assert.ok(disk.io.driver_object_id);
      assert.ok(inventory.data.some(object => object.object_id === disk.io.driver_object_id));
      assert.ok(disk.io.continuity_key);
      const filtered = await request('GET', disk.inventory_url + '&limit=500', undefined, viewer);
      assert.equal(filtered.status, 200);
      assert.ok(filtered.json().data.every(object => object.physical_disk_ids.includes(disk.object_id)));
    }
    const graph = inventory.data;
    const mounts = graph.filter(object => object.properties?.ciderd_resource_type === 'mount' && object.properties?.local === true);
    const linkedMounts = mounts.filter(object => object.physical_disk_ids.length > 0);
    assert.ok(linkedMounts.length > 0, 'A real local mount belongs to a physical disk');
    for (const asset of ['storage.js', 'storage-views.js', 'rack.js']) {
      const response = await request('GET', '/js/' + asset);
      assert.equal(response.status, 200, `Embedded ${asset} is served`);
    }
    console.log(JSON.stringify({ storage_result: 'passed', native_hardware_family: hardware.machine_family,
      physical_disks: disks.data.length, resolved_driver_associations: resolved.length,
      linked_local_mounts: linkedMounts.length, graph_objects: graph.length,
      verified_tls: true, coherent_topology: true, embedded_storage_modules: true,
      scope: 'one real local Mac; separate multi-Mac hardware acceptance remains required' }));
}

export async function verifyOfflineStorage({ node, viewer, request, daemon }) {
  if (daemon.exitCode === null && daemon.signalCode === null) {
    const exited = once(daemon, 'exit');
    daemon.kill('SIGTERM');
    await exited;
  }
  let sawDegraded = false, offline = false;
  for (let attempt = 0; attempt < 23; attempt++) {
    const result = await request('GET', `/api/v1/nodes/${node}`, undefined, viewer);
    assert.equal(result.status, 200);
    const state = result.json().data.availability;
    sawDegraded ||= state === 'degraded';
    if (state === 'offline') { offline = true; break; }
    await delay(5000);
  }
  assert.ok(sawDegraded, 'Stopped native collector transitions through degraded');
  assert.ok(offline, 'Stopped native collector becomes offline after its timeout');
  const result = await request('GET', '/api/v1/cluster', undefined, viewer);
  assert.equal(result.status, 200);
  const cluster = result.json().data;
  assert.equal(cluster.node_counts.total, 1, 'Enrolled offline node is retained');
  assert.equal(cluster.node_counts.offline, 1);
  assert.equal(cluster.observed_node_count, 0);
  for (const measurement of [cluster.capacity.local.capacity_bytes,
    cluster.throughput.read_bytes_per_second, cluster.throughput.write_bytes_per_second]) {
    assert.equal(measurement.state, 'unknown');
    assert.equal(measurement.value, null);
  }
  const disks = await request('GET', `/api/v1/nodes/${node}/disks`, undefined, viewer);
  assert.equal(disks.status, 200);
  assert.ok(disks.json().data.length > 0, 'Offline physical disks remain inspectable');
  assert.ok(disks.json().data.every(disk => disk.io.read_bytes_per_second.value === null));
  console.log(JSON.stringify({ offline_storage_result: 'passed', degraded_then_offline: true,
    retained_node_and_disks: true, current_aggregates_unknown: true }));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length > 2) throw new Error('Usage: node scripts/smoke-storage.mjs');
  await smokeCiderd({ cleanupOnFailure: true, beforeEnroll: verifyEmptyStorage,
    verify: async context => { await verifyStorage(context); await verifyOfflineStorage(context); } });
}
