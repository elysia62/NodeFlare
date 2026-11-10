import assert from 'node:assert/strict';
import { request } from 'node:http';

// The caller supplies a freshly created test service and a test-only session.
export async function testRestoreUpload(origin, cookie, derived) {
  const url = `${origin}/api/admin/database/restore?filename=test.zip`;
  const headers = { Cookie: cookie, 'X-NodeFlare-Password': derived, 'Content-Type': 'application/zip' };
  const upload = request(url, { method: 'POST', headers });
  upload.on('error', () => {}); // Client abort below is intentional.
  upload.write('PK'); // Keep a chunked upload open without buffering a large ZIP.
  try {
    let busy = false;
    for (let attempt = 0; attempt < 20; attempt++) {
      const response = await fetch(url, { method: 'POST', headers, body: 'invalid zip', signal: AbortSignal.timeout(5000) });
      if (response.status === 503) { busy = true; break; }
      assert.equal(response.status, 422);
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert(busy, 'A second authenticated restore upload must be rejected while the first owns the upload slot');
    assert.equal((await fetch(origin + '/api/bootstrap', { signal: AbortSignal.timeout(5000) })).status, 200, 'Uploading must not hold the exclusive database lock');
  } finally { upload.destroy(); }
  let released = false;
  for (let attempt = 0; attempt < 20; attempt++) {
    const response = await fetch(url, { method: 'POST', headers, body: 'invalid zip', signal: AbortSignal.timeout(5000) });
    if (response.status === 422) { released = true; break; }
    assert.equal(response.status, 503);
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  assert(released, 'Aborting the HTTP upload must release the maintenance slot');
  const backup = await fetch(origin + '/api/admin/database/backup', { headers, signal: AbortSignal.timeout(10000) });
  assert.equal(backup.status, 200, await backup.clone().text());
  await backup.arrayBuffer();
  console.log('database upload: concurrent restore rejected, reads remain available, aborted upload releases maintenance slot');
}
