import assert from 'node:assert/strict';
import { openPage } from '../../../clients/rust/tauri-shell/tests/page-harness.mjs';

const records = [
  { id: 'lead-saved-a', hostName: 'Same desktop', addedAtUnixMs: 0, lastEndpoint: { host: '192.0.2.25', tcpPort: 19730, udpPort: 19731 } },
  { id: 'lead-saved-b', hostName: 'Same desktop', addedAtUnixMs: 0, lastEndpoint: { host: 'fe80::1%5', tcpPort: 19730, udpPort: 19731 } },
];

for (const [label, width, height] of [['desktop', 1280, 800], ['mobile-width', 430, 932]]) {
  console.log('IDENTITY_QA_STEP', label, 'opening');
  const page = await openPage({ width, height });
  try {
    console.log('IDENTITY_QA_STEP', label, 'opened');
    await page.inventory([]);
    console.log('IDENTITY_QA_STEP', label, 'inventory');
    await page.evaluate(`(async () => {
      fixture.pairings = ${JSON.stringify(records)};
      await refreshSavedPairings();
    })()`);
    const cards = await page.evaluate(`[...document.querySelectorAll('.saved-pairing-card')].map(e => ({
      id: e.dataset.pairingId, text: e.innerText,
      rect: { x: e.getBoundingClientRect().x, y: e.getBoundingClientRect().y,
        width: e.getBoundingClientRect().width, height: e.getBoundingClientRect().height }
    }))`);
    assert.deepEqual(cards.map(c => c.id), records.map(r => r.id));
    console.log('IDENTITY_QA_STEP', label, 'cards');
    await page.screenshot(`.omo/pairing-20260910/evidence/identity-lead-${label}.png`);
    const selector = '[data-pairing-id="lead-saved-b"] [data-action="connect-saved"]';
    const reachability = await page.evaluate(`(() => {
      const button = document.querySelector(${JSON.stringify(selector)});
      button.scrollIntoView({ block: 'center', behavior: 'instant' });
      const r = button.getBoundingClientRect();
      const x = r.x + r.width / 2, y = r.y + r.height / 2;
      fixture.pointerTrust = null;
      button.addEventListener('click', event => { fixture.pointerTrust = event.isTrusted; }, { once: true });
      fixture.pointerArrival = fixture.command('connect', () => {});
      return {
        x, y, width: r.width, height: r.height,
        inViewport: x >= 0 && y >= 0 && x < innerWidth && y < innerHeight,
        hit: button.contains(document.elementFromPoint(x, y)),
        enabled: !button.disabled
      };
    })()`);
    assert.equal(reachability.inViewport, true);
    assert.equal(reachability.hit, true);
    assert.equal(reachability.enabled, true);
    console.log('IDENTITY_QA_STEP', label, 'reachable', JSON.stringify(reachability));
    await page.screenshot(`.omo/pairing-20260910/evidence/identity-lead-${label}-scrolled.png`);
    await page.evaluate(`(() => {
      const realm = document.createElement('iframe');
      realm.style.cssText = 'position:fixed;width:1px;height:1px;opacity:0;pointer-events:none';
      document.body.append(realm);
      fixture.pointerRealm = realm;
      fixture.frozenRaf = window.requestAnimationFrame;
      window.requestAnimationFrame = realm.contentWindow.requestAnimationFrame.bind(realm.contentWindow);
      return true;
    })()`);
    await page.view.click(selector);
    await page.evaluate(`(() => {
      window.requestAnimationFrame = fixture.frozenRaf;
      fixture.pointerRealm.remove();
      return true;
    })()`);
    console.log('IDENTITY_QA_STEP', label, 'clicked');
    const call = await page.evaluate('fixture.pointerArrival');
    const trustedClick = await page.evaluate('fixture.pointerTrust');
    assert.equal(trustedClick, true);
    assert.equal(call.args.pairingId, 'lead-saved-b');
    assert.equal(call.args.host, 'fe80::1%5');
    assert.equal(call.args.pin, null);
    assert.equal(call.args.tcpPort, 19730);
    assert.equal(call.args.udpPort, 19731);
    await page.evaluate(`fixture.until(
      () => document.getElementById('viewport-container').dataset.phase === 'waiting-video',
      () => fixture.settle('connect', null))`);
    await page.evaluate(`fixture.command('disconnect', () => { fixture.stop = doDisconnect(); })`);
    await page.evaluate(`fixture.until(
      () => document.getElementById('viewport-container').dataset.phase === 'idle',
      () => fixture.settle('disconnect', null))`);
    console.log('IDENTITY_PAGE_PASS', JSON.stringify({ label, width, height, cards, reachability, trustedClick, call }));
  } finally {
    await page.close();
  }
}
