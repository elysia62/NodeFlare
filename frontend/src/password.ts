const CLIENT_PASSWORD_ROUNDS = 600_000;
const encoder = new TextEncoder();

function toHex(bytes: Uint8Array) {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export async function derivePassword(password: string, deploymentSalt: string, locale?: string) {
  const english = locale === "en";
  if (!deploymentSalt) throw new Error(english ? "Password derivation parameters are missing" : "登录密码派生参数缺失");
  if (!globalThis.crypto?.subtle) throw new Error(english ? "This browser does not support secure password derivation" : "当前浏览器不支持安全密码派生");
  const key = await globalThis.crypto.subtle.importKey(
    "raw",
    encoder.encode(password),
    "PBKDF2",
    false,
    ["deriveBits"],
  );
  const bits = await globalThis.crypto.subtle.deriveBits(
    {
      name: "PBKDF2",
      hash: "SHA-256",
      iterations: CLIENT_PASSWORD_ROUNDS,
      salt: encoder.encode(`nodeflare:${deploymentSalt}`),
    },
    key,
    256,
  );
  return toHex(new Uint8Array(bits));
}
