import { html } from './lib.js';
import { statusColor } from './data.js';
import { timeLabel } from './model.js';
import { findingVM, ruleFindingVM, securityPageVM, sourceVM } from './security.js';
import { PageControls } from './views.js';

const Panel = ({ title, note, children }) => html`<section class="panel security-panel"><div class="panel-head"><h3>${title}</h3>${note ? html`<span class="note">${note}</span>` : null}</div><div class="panel-pad">${children}</div></section>`;
const Tag = ({ state }) => html`<span class="tag sm" style=${{ color: statusColor(state) }}>${state || 'unknown'}</span>`;
const Empty = ({ children }) => html`<p class="empty-note">${children}</p>`;
const nodeName = (nodes, id) => nodes.find(node => (node.id || node.node_id) === id)?.name || id;
const nodeLink = (id, objectId = null) => `#node/${encodeURIComponent(id)}${objectId ? `/object/${encodeURIComponent(objectId)}` : ''}`;
const policyName = key => key.replaceAll('_', ' ');
const policyValue = value => typeof value === 'number' && Number.isFinite(value)
  ? value.toLocaleString() : typeof value === 'string' || typeof value === 'boolean' ? String(value) : 'Unknown';

const PageState = ({ kind, paging }) => {
  const page = securityPageVM(kind, paging);
  return html`<div class="security-page-state"><${PageControls} paging=${paging}/>
    ${page.contractError ? html`<p class="connection-error" role="status">${page.contractError} Valid dated rows remain visible.</p>` : null}
  </div>`;
};

const EvidenceInterval = ({ label, interval }) => !interval ? html`<div class="security-evidence-card"><h5>${label}</h5><p class="note">Interval evidence unavailable.</p></div>` : html`<div class="security-evidence-card">
  <h5>${label}</h5>
  <div class="security-evidence-rate">${interval.rateLabel}</div>
  <dl class="security-details">
    <div><dt>Interval</dt><dd>${interval.intervalLabel}</dd></div>
    <div><dt>Observed</dt><dd>${interval.observedLabel}</dd></div>
    <div><dt>Received</dt><dd>${interval.receivedLabel}</dd></div>
    <div><dt>Counter start</dt><dd><code>${interval.counterStart}</code> bytes</dd></div>
    <div><dt>Counter end</dt><dd><code>${interval.counterEnd}</code> bytes</dd></div>
    <div><dt>Monotonic range</dt><dd><code>${interval.monotonicStart}</code>–<code>${interval.monotonicEnd}</code> ns</dd></div>
    <div><dt>Collections</dt><dd><code>${interval.previousCollectionId}</code> → <code>${interval.collectionId}</code></dd></div>
  </dl>
  <details class="security-continuity"><summary>Continuity evidence</summary><dl class="security-details">${Object.entries(interval.continuity).map(([key, value]) => html`<div key=${key}><dt>${policyName(key)}</dt><dd><code>${String(value)}</code></dd></div>`)}</dl></details>
</div>`;

const FindingCard = ({ finding, nodes }) => {
  const vm = finding.valid === true ? finding : findingVM(finding);
  return html`<article class=${`security-finding finding-${vm.status}`}>
    <header><div><div class="eyebrow-accent">${vm.direction} · ${vm.ruleId}</div><h4>${vm.summary}</h4></div><${Tag} state=${vm.status === 'open' ? 'warning' : vm.status}/></header>
    <p class="security-links"><a href=${nodeLink(vm.nodeId)}>${nodeName(nodes, vm.nodeId)}</a><span>Driver source <a href=${nodeLink(vm.nodeId, vm.objectId)} title=${`Open host storage context for source object ${vm.objectId}`}><code>${vm.objectId}</code></a></span></p>
    <p class="note">This finding is scoped to the reported driver object. It does not identify a physical disk, file, process, user, or unauthorized action.</p>
    <dl class="security-details">
      <div><dt>Lifecycle</dt><dd>${vm.status} · ${vm.reasonLabel}</dd></div>
      <div><dt>First / latest evidence</dt><dd>${vm.firstSeenLabel} / ${vm.lastSeenLabel}</dd></div>
      <div><dt>Opened / updated</dt><dd>${vm.openedLabel} / ${vm.updatedLabel}</dd></div>
      <div><dt>Frozen comparison</dt><dd>${vm.baselineLabel}</dd></div>
      <div><dt>Frozen coverage</dt><dd>${vm.baselineCoverageLabel} · revision ${vm.baselineRevision}</dd></div>
      <div><dt>Episode duration</dt><dd>${vm.elevatedLabel} · ${vm.recoveryLabel}</dd></div>
    </dl>
    <details class="security-finding-evidence"><summary>Expand frozen interval evidence</summary>
      <p class="note">These counter endpoints, thresholds, and dates were retained with the finding. They do not update while this page is being reviewed.</p>
      <div class="security-evidence-grid"><${EvidenceInterval} label="First qualifying interval" interval=${vm.evidence.first}/><${EvidenceInterval} label="Latest interval" interval=${vm.evidence.latest}/><${EvidenceInterval} label="Peak interval" interval=${vm.evidence.peak}/></div>
      <details><summary>Rule policy at opening</summary><dl class="security-details">${Object.entries(vm.policy).map(([key, value]) => html`<div key=${key}><dt>${policyName(key)}</dt><dd>${policyValue(value)}</dd></div>`)}</dl></details>
    </details>
  </article>`;
};

const Findings = ({ paging = {}, nodes }) => {
  const page = securityPageVM('findings', paging);
  return html`<${Panel} title="Generated storage-activity findings" note="Durable detector evidence">
    <p>Each finding records sustained read or write activity above that driver source's own learned reference. It is evidence for administrator assessment, not a conclusion that a node was compromised.</p>
    <${PageState} kind="findings" paging=${paging}/>
    ${page.rows.length ? html`<div class="security-findings">${page.rows.map(row => html`<${FindingCard} key=${row.findingId} finding=${row} nodes=${nodes}/>` )}</div>`
      : html`<${Empty}>${paging.busy ? 'Loading generated findings…' : paging.error ? 'No finding snapshot is available. Previously dated evidence, when present, remains visible.' : 'No generated findings are present in this snapshot. This does not establish that storage activity is safe or healthy.'}<//>`}
  <//>`;
};

const SourceRow = ({ source, nodes }) => {
  const vm = source.valid === true ? source : sourceVM(source);
  return html`<tr key=${vm.sourceId}>
    <td><a href=${nodeLink(vm.nodeId)}>${nodeName(nodes, vm.nodeId)}</a><div class="note"><code>${vm.nodeId}</code></div></td>
    <td><a href=${nodeLink(vm.nodeId, vm.objectId)} title=${`Open host storage context for source object ${vm.objectId}`}><code>${vm.objectId}</code></a><div class="note">Driver object · ${vm.direction} · ${vm.metric}</div><div class="note">Source <code>${vm.sourceId}</code></div></td>
    <td><${Tag} state=${vm.supportState}/><div>${vm.supportLabel} · ${!vm.active ? 'Inactive' : vm.supportState === 'supported' ? 'Admitted' : 'Not admitted'}</div><div class="note">Policy ${vm.policyVersion} · baseline revision ${vm.baselineRevision}</div></td>
    <td><${Tag} state=${vm.baselineState}/><div>${vm.readinessLabel}</div><div class="note">${vm.readinessAsOfLabel}</div><div class="note">${vm.thresholdLabel}</div></td>
    <td><${Tag} state=${vm.episodeState === 'open' ? 'warning' : vm.episodeState}/><div>${vm.episodeLabel}</div></td>
    <td><${Tag} state=${vm.observationState}/><div>${vm.observationLabel}</div></td>
  </tr>`;
};

const Sources = ({ paging = {}, nodes, now, wallNow }) => {
  const page = securityPageVM('sources', paging, now, wallNow);
  const ready = page.rows.filter(row => row.baselineState === 'ready').length;
  const learning = page.rows.filter(row => row.baselineState === 'learning').length;
  const unavailable = page.rows.filter(row => row.observationState !== 'current').length;
  const policy = paging.meta?.policy && typeof paging.meta.policy === 'object' ? paging.meta.policy : null;
  const coverage = paging.meta?.coverage && typeof paging.meta.coverage === 'object' ? paging.meta.coverage : null;
  return html`<${Panel} title="Detector source coverage" note="Read and write evaluated independently">
    <p>Coverage describes which native driver directions are admitted and ready. Learning, unsupported, capacity-limited, inactive, stale, and unavailable sources remain explicit. A learned baseline describes observed activity; it does not prove that activity was authorized.</p>
    <p class="note">Readiness and coverage are frozen at each row's evaluation time. Observation currentness ages locally from the browser's monotonic and wall clocks starting when the snapshot request begins; a slow or failed refresh cannot extend a current label.</p>
    ${policy ? html`<details class="security-policy"><summary>Current detector policy</summary><dl class="security-details">${Object.entries(policy).map(([key, value]) => html`<div key=${key}><dt>${policyName(key)}</dt><dd>${policyValue(value)}</dd></div>`)}</dl><p class="note">Policy parameters are engineering defaults, not calibrated sensitivity, false-positive rates, or a guarantee that an attack is detectable.</p></details>` : html`<p class="connection-error" role="status">Detector policy metadata is unavailable for this snapshot.</p>`}
    ${coverage ? html`<details class="security-coverage" open><summary>Snapshot-wide coverage</summary><dl class="security-details">${Object.entries(coverage).map(([key, value]) => html`<div key=${key}><dt>${policyName(key)}</dt><dd>${policyValue(value)}</dd></div>`)}</dl></details>` : html`<p class="connection-error" role="status">Snapshot-wide detector coverage metadata is unavailable.</p>`}
    <div class="security-kpis" aria-label="Detector coverage on this page"><div><strong>${page.rows.length}</strong><span>sources on page</span></div><div><strong>${ready}</strong><span>ready</span></div><div><strong>${learning}</strong><span>learning</span></div><div><strong>${unavailable}</strong><span>not current</span></div></div>
    <${PageState} kind="sources" paging=${paging}/>
    ${page.rows.length ? html`<div class="table-scroll"><table class="live-table security-sources"><thead><tr><th>Node</th><th>Driver source</th><th>Support</th><th>Baseline readiness</th><th>Episode</th><th>Observation</th></tr></thead><tbody>${page.rows.map(row => html`<${SourceRow} key=${row.sourceId} source=${row} nodes=${nodes}/>` )}</tbody></table></div>`
      : html`<${Empty}>${paging.busy ? 'Loading detector source coverage…' : paging.error ? 'Detector source coverage is unavailable. Do not infer support or readiness.' : 'No detector sources are reported in this snapshot. Coverage and readiness are unknown.'}<//>`}
  <//>`;
};

const CollectorEvents = ({ paging = {}, nodes }) => {
  const page = securityPageVM('events', paging);
  return html`<${Panel} title="Collector-reported security events" note="Separate from generated findings">
    <p>These events were explicitly reported by collectors and retain their original source. They are not detector findings and have a separate frozen cursor snapshot.</p>
    <${PageState} kind="events" paging=${paging}/>
    ${page.rows.length ? html`<div class="table-scroll"><table class="live-table"><thead><tr><th>Observed</th><th>Severity</th><th>Source</th><th>Summary</th></tr></thead><tbody>${page.rows.map(event => html`<tr key=${event.event_id}><td>${timeLabel(event.occurred_at)}</td><td><${Tag} state=${event.severity}/></td><td>${event.source}<div class="note"><a href=${nodeLink(event.node_id)}>${nodeName(nodes, event.node_id)}</a></div></td><td>${event.summary}</td></tr>`)}</tbody></table></div>`
      : html`<${Empty}>${paging.busy ? 'Loading collector events…' : paging.error ? 'No collector-event snapshot is available.' : 'No collector-reported security events are present. This does not establish that no security activity occurred.'}<//>`}
  <//>`;
};

const FeatureList = ({ features = {} }) => html`<dl class="security-details">${Object.entries(features).map(([key, value]) => html`<div key=${key}><dt>${policyName(key)}</dt><dd>${typeof value === 'number' ? value.toLocaleString(undefined, { maximumFractionDigits: 2 }) : String(value)}</dd></div>`)}</dl>`;

const RuleFindings = ({ paging = {}, nodes }) => {
  const page = securityPageVM('ruleFindings', paging);
  return html`<${Panel} title="Filesystem and NFS rule findings" note="Fixed thresholds · server evaluated">
    <p>These findings use explicit operation-rate, throughput, mount-health, and snapshot rules. They identify behavior that needs review; they do not prove malicious intent.</p>
    <${PageState} kind="ruleFindings" paging=${paging}/>
    ${page.rows.length ? html`<div class="security-findings">${page.rows.map(row => {
      const vm = row.valid === true ? row : ruleFindingVM(row);
      return html`<article class=${`security-finding finding-${vm.status}`} key=${vm.findingId}>
        <header><div><div class="eyebrow-accent">${vm.scope} · ${vm.ruleId}</div><h4>${vm.summary}</h4></div><${Tag} state=${vm.status === 'open' ? vm.severity : vm.status}/></header>
        <p class="security-links"><a href=${nodeLink(vm.nodeId)}>${nodeName(nodes, vm.nodeId)}</a><span><a href=${nodeLink(vm.nodeId, vm.objectId)}><code>${vm.objectId}</code></a></span></p>
        <dl class="security-details"><div><dt>First / latest evidence</dt><dd>${vm.firstSeenLabel} / ${vm.lastSeenLabel}</dd></div><div><dt>Updated</dt><dd>${vm.updatedLabel}</dd></div><div><dt>Rule timing</dt><dd>${vm.requiredIntervals ?? 'unknown'} qualifying intervals · ${vm.recoveryIntervals ?? 'unknown'} recovery intervals</dd></div></dl>
        <details><summary>Normalized feature evidence</summary><${FeatureList} features=${vm.features}/></details>
      </article>`;
    })}</div>` : html`<${Empty}>${paging.busy ? 'Loading rule findings…' : paging.error ? 'Rule findings are unavailable.' : 'No fixed-rule filesystem or NFS findings have been generated.'}<//>`}
  <//>`;
};

const RuleSources = ({ paging = {}, nodes }) => {
  const page = securityPageVM('ruleSources', paging);
  return html`<${Panel} title="Rule inputs" note="Model-ready feature snapshots">
    <p>Latest normalized server-side inputs are retained here so operators can audit each decision and later models can consume the same feature names.</p>
    <${PageState} kind="ruleSources" paging=${paging}/>
    ${page.rows.length ? html`<div class="table-scroll"><table class="live-table"><thead><tr><th>Source</th><th>Observation</th><th>Rule state</th><th>Latest features</th></tr></thead><tbody>${page.rows.map(vm => html`<tr key=${vm.sourceId}><td><a href=${nodeLink(vm.nodeId)}>${nodeName(nodes, vm.nodeId)}</a><div class="note">${vm.scope} · ${vm.collector}</div></td><td><${Tag} state=${vm.observationState}/><div class="note">${vm.observedLabel}</div></td><td>${vm.rules.filter(rule => rule.state === 'open').length ? `${vm.rules.filter(rule => rule.state === 'open').length} open` : 'Quiet'}<div class="note">${vm.rules.length} rules evaluated</div></td><td><${FeatureList} features=${vm.features}/></td></tr>`)}</tbody></table></div>`
      : html`<${Empty}>${paging.busy ? 'Waiting for rule inputs…' : paging.error ? 'Rule inputs are unavailable.' : 'No compatible filesystem or NFS telemetry has arrived yet.'}<//>`}
  <//>`;
};

export const SecurityView = ({ security = {}, nodes = [], now = performance.now(), wallNow = Date.now() }) => html`<div class="stack security-view">
  <${RuleFindings} paging=${security.ruleFindings || { rows: [], busy: true }} nodes=${nodes}/>
  <${RuleSources} paging=${security.ruleSources || { rows: [], busy: true }} nodes=${nodes}/>
  <${Panel} title="Storage activity detection" note="Read-only viewer">
    <p>Orchard compares each admitted native driver's read and write rates with that same source's learned history. Findings preserve the threshold, counter intervals, dates, and coverage used at detection time.</p>
    <p class="note">This first rule observes sustained upper-rate deviations. It does not identify file access, application throughput, or a physical disk association, and it does not establish that quieter activity is safe. Administrator relearning is intentionally outside this viewer.</p>
  <//>
  <${Findings} paging=${security.findings || { rows: [], busy: true }} nodes=${nodes}/>
  <${Sources} paging=${security.sources || { rows: [], busy: true }} nodes=${nodes} now=${now} wallNow=${wallNow}/>
  <${CollectorEvents} paging=${security.events || { rows: [], busy: true }} nodes=${nodes}/>
</div>`;
