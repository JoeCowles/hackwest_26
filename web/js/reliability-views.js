import { html } from './lib.js';
import { timeLabel } from './model.js';
import { reliabilityVM } from './reliability.js';

const words = value => typeof value === 'string' && value.length ? value.replaceAll('_', ' ') : 'unknown';
const sourceLink = source => source.nodeId && source.objectId
  ? `#node/${encodeURIComponent(source.nodeId)}/object/${encodeURIComponent(source.objectId)}` : null;
const exactJSON = value => JSON.stringify(value, null, 2);
const badgeClass = value => `reliability-badge reliability-${String(value || 'unknown').replaceAll('_', '-')}`;

const Observation = ({ value }) => html`<div class="reliability-observation">
  <span class=${badgeClass(value.state)}>${value.state}</span>
  <span>${value.label}</span>
  <span class="note">Observed ${value.observedLabel} · received ${value.receivedLabel}</span>
</div>`;

const Signal = ({ signal }) => html`<article class="reliability-signal">
  <header><strong>${signal.title}</strong><span class=${badgeClass(signal.severity)}>${signal.severity}</span></header>
  <p class="note">${signal.ruleId} · ${words(signal.dimension)} · ${words(signal.state)}</p>
  ${signal.dimension === 'performance' ? html`<dl class="reliability-details">
    <div><dt>Baseline readiness</dt><dd>${signal.readinessLabel}</dd></div>
    <div><dt>Thresholds</dt><dd>${signal.thresholdLabel}</dd></div>
    <div><dt>Comparable workload</dt><dd>${signal.workloadLabel}</dd></div>
    <div><dt>Episode</dt><dd>${signal.episodeLabel}</dd></div>
  </dl>` : null}
  <${Observation} value=${signal.observation}/>
  ${signal.evidence ? html`<details><summary>Exact signal evidence</summary><pre class="storage-json">${exactJSON(signal.evidence)}</pre></details>` : null}
</article>`;

const Source = ({ source }) => {
  const link = sourceLink(source);
  return html`<article class="reliability-source">
    <header><div><strong>${source.collector}</strong><div class="note"><code>${source.sourceId || 'Source ID not reported'}</code></div></div><span class=${badgeClass(source.observationState)}>${source.observationState}</span></header>
    <dl class="reliability-details">
      <div><dt>Scope</dt><dd>${source.scope}</dd></div>
      <div><dt>Resource</dt><dd><code>${source.resourceId || 'Not reported'}</code></dd></div>
      <div><dt>Identity confidence</dt><dd>${source.confidence ? words(source.confidence) : 'Not reported'}</dd></div>
      <div><dt>Signals</dt><dd>${source.signals.length}</dd></div>
    </dl>
    ${link ? html`<p><a href=${link}>Open source object <code>${source.objectId}</code></a></p>` : html`<p class="note">Source object link unavailable.</p>`}
    <${Observation} value=${source.observation}/>
    ${source.policy ? html`<details><summary>Source rule policy</summary><pre class="storage-json">${exactJSON(source.policy)}</pre></details>` : null}
  </article>`;
};

const Finding = ({ finding }) => {
  const link = sourceLink(finding);
  return html`<article class=${`reliability-finding reliability-finding-${finding.status}`}>
    <header><div><div class="eyebrow-accent">${words(finding.classification)} · ${words(finding.dimension)}</div><h5>${finding.summary}</h5></div><span class=${badgeClass(finding.severity)}>${finding.severity}</span></header>
    <p><strong>${finding.current ? 'Current finding' : 'Dated evidence'}</strong> · ${words(finding.status)} · ${finding.ruleId}</p>
    ${link ? html`<p><a href=${link}>Open source object <code>${finding.objectId}</code></a></p>` : null}
    <dl class="reliability-details">
      <div><dt>First / latest evidence</dt><dd>${finding.firstSeenLabel} / ${finding.lastSeenLabel}</dd></div>
      <div><dt>Opened / updated</dt><dd>${finding.openedLabel} / ${finding.updatedLabel}</dd></div>
      <div><dt>Ended</dt><dd>${finding.endedLabel} · ${finding.reasonLabel}</dd></div>
      <div><dt>Source</dt><dd><code>${finding.sourceId || 'Not reported'}</code></dd></div>
      <div><dt>Resource</dt><dd><code>${finding.resourceId || 'Not reported'}</code></dd></div>
      <div><dt>Policy version</dt><dd>${finding.policyVersion}</dd></div>
      <div><dt>Scope</dt><dd>${words(finding.scope)}</dd></div>
      <div><dt>Identity confidence</dt><dd>${finding.confidence ? words(finding.confidence) : 'Not reported'}</dd></div>
      <div><dt>Finding ID</dt><dd><code>${finding.findingId || 'Not reported'}</code></dd></div>
    </dl>
    <details class="reliability-finding-evidence"><summary>Expand exact finding evidence</summary>
      <p class="note">Exact counters, thresholds and dates are retained with this finding. Dated evidence remains inspectable when current telemetry is stale or unavailable.</p>
      ${finding.evidence ? html`<pre class="storage-json">${exactJSON(finding.evidence)}</pre>` : html`<p class="note">Finding evidence unavailable.</p>`}
    </details>
  </article>`;
};

const signalsFor = (vm, kind) => vm.signals.filter(signal => {
  if (kind === 'performance') return signal.dimension === 'performance';
  const direct = /overall|critical_warning|pending|uncorrectable|spare|endurance/i.test(signal.ruleId);
  return kind === 'condition' ? direct : signal.dimension !== 'performance' && !direct;
});

const SignalSection = ({ title, signals, empty }) => html`<section class="reliability-section"><h5>${title}</h5>
  ${signals.length ? html`<div class="reliability-signal-grid">${signals.map((signal, index) => html`<${Signal} key=${signal.source.sourceId + signal.ruleId + index} signal=${signal}/>` )}</div>` : html`<p class="note">${empty}</p>`}
</section>`;

export function ReliabilityPanel({ reliability, snapshotAgeMs = 0, current = false }) {
  const vm = reliabilityVM(reliability, { snapshotAgeMs, current });
  const conditions = signalsFor(vm, 'condition'), deterioration = signalsFor(vm, 'deterioration');
  const performance = signalsFor(vm, 'performance');
  return html`<section class="reliability-panel" aria-labelledby="drive-reliability-heading">
    <header class="reliability-panel-head"><div><div class="eyebrow-accent">Passive observations</div><h4 id="drive-reliability-heading">Drive reliability evidence</h4></div><span class=${badgeClass(vm.assessment)}>${vm.assessmentLabel}</span></header>
    <div class="reliability-kpis">
      <div><span>Observation</span><strong>${words(vm.observationState)}</strong></div>
      <div><span>Source coverage</span><strong>${vm.coverageLabel}</strong></div>
      <div><span>Replacement forecast</span><strong>${vm.forecastLabel}</strong></div>
    </div>
    ${!vm.available ? html`<p class="empty-note">This disk summary does not include reliability evidence. Reported conditions, deterioration, performance readiness and replacement timing are unavailable.</p>` : null}
    <p class="note">Unknown dimensions: ${vm.unknownDimensions.length ? `${vm.unknownDimensions.map(words).join(', ')} · unknown` : 'none reported'}. No current warning applies only to observed dimensions and is not a blanket healthy assessment.</p>
    <${SignalSection} title="Reported conditions" signals=${conditions} empty="No reported condition signal is available in this snapshot."/>
    <${SignalSection} title="Deterioration and observed errors" signals=${deterioration} empty="No deterioration or counter-error signal is available in this snapshot."/>
    <section class="reliability-section"><h5>Service-time baseline readiness</h5>
      ${performance.length ? html`<div class="reliability-readiness-list">
        ${performance.map((signal, index) => html`<${Signal} key=${signal.source.sourceId + signal.ruleId + index} signal=${signal}/>`)}
      </div>` : html`<p class="note">Driver-accounted service-time baseline readiness is unavailable. Workload, caching and controller effects remain possible causes of elevated service time.</p>`}
    </section>
    <section class="reliability-section"><h5>Source inventory and confidence</h5>
      <p class="note">${vm.coverageLabel}. Each source ages by its own reported freshness limit.</p>
      ${vm.sources.length ? html`<div class="reliability-source-grid">${vm.sources.map((source, index) => html`<${Source} key=${source.sourceId || index} source=${source}/>` )}</div>` : html`<p class="note">No reliability sources were reported for this disk.</p>`}
    </section>
    <section class="reliability-section"><h5>Durable findings</h5>
      ${vm.findings.length ? html`<div class="reliability-findings">${vm.findings.map((finding, index) => html`<${Finding} key=${finding.findingId || index} finding=${finding}/>` )}</div>` : html`<p class="note">No durable findings were included in this disk snapshot. This does not establish that unobserved dimensions are healthy.</p>`}
    </section>
    ${vm.forecast?.estimated_failure_at ? html`<p class="note">Forecast date: ${timeLabel(vm.forecast.estimated_failure_at)}</p>` : null}
  </section>`;
}
