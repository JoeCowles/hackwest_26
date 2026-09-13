// Passive real-Mac reliability admission. Fault conditions use deterministic tests.
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { smokeCiderd } from './smoke-ciderd.mjs';

await smokeCiderd({
  cleanupOnFailure: true,
  beforeEnroll: async ({ request }) => {
    for (const path of ['/api/v1/reliability/sources', '/api/v1/reliability/findings']) {
      const result = await request('GET', path);
      assert.equal(result.status, 200);
      assert.deepEqual(result.json().data, []);
    }
  },
  verify: async ({ node, viewer, request, daemon }) => {
    const path = `/api/v1/reliability/sources?node_id=${node}`;
    let sources = [];
    for (let attempt = 0; attempt < 6; attempt++) {
      const result = await request('GET', path, undefined, viewer);
      assert.equal(result.status, 200);
      sources = result.json().data;
      if (sources.some(source => source.collector === 'iokit.block' && source.observation.state === 'current')) break;
      await delay(5000);
    }
    const current = sources.filter(source => source.collector === 'iokit.block' && source.observation.state === 'current');
    assert.ok(current.length, 'Real IOKit reliability sources reached the server');
    for (const source of current) {
      assert.equal(source.node_id, node);
      assert.equal(source.support_state, 'supported');
      assert.ok(source.signals.some(signal => signal.rule_id === 'iokit.read_service_time'));
      assert.equal(source.replacement_forecast.state, 'insufficient_data');
    }
    const disks = await request('GET', `/api/v1/nodes/${node}/disks`, undefined, viewer);
    assert.equal(disks.status, 200);
    for (const disk of disks.json().data) {
      assert.equal(disk.reliability.replacement_forecast.state, 'insufficient_data');
      assert.ok(Array.isArray(disk.reliability.unknown_dimensions));
    }
    for (const asset of ['reliability.js', 'reliability-views.js']) assert.equal((await request('GET', `/js/${asset}`)).status, 200);
    if (daemon.exitCode === null && daemon.signalCode === null) { const exited = once(daemon, 'exit'); daemon.kill('SIGTERM'); await exited; }
    await delay(16_000);
    const stopped = await request('GET', path, undefined, viewer);
    assert.equal(stopped.status, 200);
    assert.ok(stopped.json().data.every(source => source.observation.state !== 'current'));
    console.log(JSON.stringify({ reliability_result: 'passed', verified_tls: true,
      native_iokit_sources: current.length, physical_disks: disks.json().data.length,
      forecast_unknown: true, stopped_collector_not_current: true, embedded_reliability_modules: true,
      scope: 'Passive real-Mac admission and aging; SMART optional and disabled in example configuration; synthetic tests cover faults and sustained degradation.' }));
  }
});
