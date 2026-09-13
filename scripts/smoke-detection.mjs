// Real native admission/learning smoke; deterministic duration/episode cases are
// covered by Rust tests. Uses the disposable verified-TLS smoke-ciderd harness.
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { smokeCiderd } from './smoke-ciderd.mjs';

await smokeCiderd({
  cleanupOnFailure: true,
  beforeEnroll: async ({ request }) => {
    const sources = await request('GET', '/api/v1/detectors/storage-activity/sources');
    assert.equal(sources.status, 200);
    assert.deepEqual(sources.json().data, []);
    assert.equal(sources.json().meta.coverage.security_assessment, 'unknown');
    const findings = await request('GET', '/api/v1/findings');
    assert.equal(findings.status, 200);
    assert.deepEqual(findings.json().data, []);
  },
  verify: async ({ node, viewer, request, daemon }) => {
    const path = `/api/v1/detectors/storage-activity/sources?node_id=${node}`;
    let sources;
    for (let attempt = 0; attempt < 8; attempt++) {
      const result = await request('GET', path, undefined, viewer);
      assert.equal(result.status, 200);
      sources = result.json();
      if (['read', 'write'].every(direction => sources.data.some(source =>
        source.direction === direction && source.observation.state === 'current'))) break;
      await delay(5000);
    }
    assert.ok(sources.data.length >= 2, 'Native driver read and write sources are admitted');
    for (const direction of ['read', 'write']) {
      const current = sources.data.find(source => source.direction === direction && source.observation.state === 'current');
      assert.ok(current, `Real ${direction} counter intervals reached detection`);
      assert.equal(current.node_id, node);
      assert.equal(current.support_state, 'supported');
      assert.equal(current.baseline.state, 'learning');
      assert.equal(current.baseline.required_seconds, 600);
      assert.ok(current.baseline.covered_seconds > 0);
      assert.ok(current.baseline.as_of);
      assert.equal(current.episode.state, 'quiet');
    }
    const findings = await request('GET', '/api/v1/findings', undefined, viewer);
    assert.equal(findings.status, 200);
    assert.deepEqual(findings.json().data, [], 'Short learning smoke cannot establish a baseline or open a finding');
    const capabilities = (await request('GET', '/api/v1/capabilities', undefined, viewer)).json();
    assert.equal(capabilities.data.features.storage_activity_detection, true);
    assert.equal(capabilities.data.features.findings, true);
    assert.equal(capabilities.data.features.alerts, false);
    assert.equal(capabilities.data.features.twilio_notifications, false);
    for (const asset of ['security.js', 'security-views.js']) {
      assert.equal((await request('GET', `/js/${asset}`)).status, 200);
    }
    if (daemon.exitCode === null && daemon.signalCode === null) {
      const exited = once(daemon, 'exit'); daemon.kill('SIGTERM'); await exited;
    }
    await delay(16_000);
    const stopped = await request('GET', path, undefined, viewer);
    assert.equal(stopped.status, 200);
    assert.ok(stopped.json().data.every(source => source.observation.state !== 'current'));
    console.log(JSON.stringify({ detection_result:'passed', verified_tls:true,
      native_directions:['read','write'], admitted_sources:sources.data.length,
      learning_observed:true, stopped_collector_not_current:true, embedded_security_modules:true,
      scope:'one real Mac admission and learning; no real-duration anomaly or detection efficacy claim' }));
  }
});
