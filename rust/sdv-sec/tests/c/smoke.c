/* Calls the C ABI through the public header, the way an ECU written in C/C++ would. */
#include "sdv_sec.h"

#include <stdio.h>
#include <string.h>

static int failures;
#define CHECK(cond)                                                     \
  do {                                                                  \
    if (!(cond)) {                                                      \
      printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond);            \
      failures++;                                                       \
    }                                                                   \
  } while (0)

static void from_hex(const char* s, uint8_t* out, size_t n) {
  for (size_t i = 0; i < n; i++) {
    unsigned v;
    sscanf(s + 2 * i, "%2x", &v);
    out[i] = (uint8_t)v;
  }
}

int main(void) {
  uint8_t out[32], want[64];

  /* SHA-256("abc") */
  CHECK(sdv_sec_sha256((const uint8_t*)"abc", 3, out) == SDV_SEC_OK);
  from_hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", want, 32);
  CHECK(memcmp(out, want, 32) == 0);
  CHECK(sdv_sec_sha256(NULL, 0, out) == SDV_SEC_OK); /* empty input may be null */
  CHECK(sdv_sec_sha256(NULL, 3, out) == SDV_SEC_INVALID_ARG);
  CHECK(sdv_sec_sha256((const uint8_t*)"abc", 3, NULL) == SDV_SEC_INVALID_ARG);

  /* HMAC-SHA256, RFC 4231 test case 2 */
  CHECK(sdv_sec_hmac_sha256((const uint8_t*)"Jefe", 4, (const uint8_t*)"what do ya want for nothing?", 28, out) == SDV_SEC_OK);
  from_hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843", want, 32);
  CHECK(memcmp(out, want, 32) == 0);
  CHECK(sdv_sec_ct_eq(out, want, 32) == 1);
  want[5] ^= 1;
  CHECK(sdv_sec_ct_eq(out, want, 32) == 0);

  /* Ed25519, RFC 8032 section 7.1 test 2 */
  uint8_t pk[32], sig[64], msg[1] = {0x72};
  from_hex("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c", pk, 32);
  from_hex("92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00", sig, 64);
  CHECK(sdv_sec_ed25519_verify(pk, msg, 1, sig) == SDV_SEC_OK);
  msg[0] = 0x73;
  CHECK(sdv_sec_ed25519_verify(pk, msg, 1, sig) == SDV_SEC_BAD_SIGNATURE);
  msg[0] = 0x72;
  sig[10] ^= 1;
  CHECK(sdv_sec_ed25519_verify(pk, msg, 1, sig) == SDV_SEC_BAD_SIGNATURE);
  sig[10] ^= 1;
  CHECK(sdv_sec_ed25519_verify(NULL, msg, 1, sig) == SDV_SEC_INVALID_ARG);

  /* frame MAC and freshness */
  const uint8_t key[] = "per-ecu demo key";
  const uint8_t payload[] = {1, 2, 3};
  uint8_t tag[SDV_SEC_FRAME_MAC_LEN];
  CHECK(sdv_sec_frame_mac(key, sizeof key - 1, 0x10, 7, payload, 3, tag) == SDV_SEC_OK);
  CHECK(sdv_sec_frame_mac_verify(key, sizeof key - 1, 0x10, 7, payload, 3, tag) == SDV_SEC_OK);
  CHECK(sdv_sec_frame_mac_verify(key, sizeof key - 1, 0x10, 8, payload, 3, tag) == SDV_SEC_BAD_TAG);
  CHECK(sdv_sec_frame_mac_verify(key, sizeof key - 1, 0x11, 7, payload, 3, tag) == SDV_SEC_BAD_TAG);

  uint32_t last = 10;
  CHECK(sdv_sec_freshness_accept(&last, 10) == SDV_SEC_STALE);
  CHECK(last == 10);
  CHECK(sdv_sec_freshness_accept(&last, 11) == SDV_SEC_OK);
  CHECK(last == 11);
  CHECK(sdv_sec_freshness_accept(NULL, 12) == SDV_SEC_INVALID_ARG);

  printf(failures ? "%d check(s) failed\n" : "C ABI smoke test: all checks passed\n", failures);
  return failures ? 1 : 0;
}
