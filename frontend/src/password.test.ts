import { expect, test } from 'bun:test';
import vectors from '../../shared/password-vectors.json';
import { derivePassword } from './password';

test('WebCrypto password derivation matches the server UTF-8 vectors', async () => {
  for (const vector of vectors) {
    expect(await derivePassword(vector.password, vector.deployment_salt)).toBe(vector.derived);
  }
});
