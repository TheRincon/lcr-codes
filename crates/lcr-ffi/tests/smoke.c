/*
 * The C API from C: does include/lcr.h describe the library the way the library behaves?
 *
 * tests/api.rs exercises the same functions from Rust, which proves the behaviour but not
 * the header — a field order or a type width that disagrees with the Rust side compiles
 * fine there and corrupts memory here. This program is built with warnings as errors
 * against the committed header and linked against the static library, and `just ffi-test`
 * runs it.
 *
 * It is also the shortest complete example of the API: encode, decode, render the body
 * as JSON, combine frames, split a file into symbols and reassemble it.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "lcr.h"

static int failures = 0;

#define CHECK(cond)                                                                    \
  do {                                                                                 \
    if (!(cond)) {                                                                     \
      const char *e = lcr_last_error();                                                \
      fprintf(stderr, "%s:%d: CHECK(%s) failed (last error: %s)\n", __FILE__, __LINE__, \
              #cond, e ? e : "none");                                                  \
      failures++;                                                                      \
    }                                                                                  \
  } while (0)

static LcrImage image_of(const LcrRendered *r) {
  LcrImage img = {r->pixels.data, r->pixels.len, r->width, r->height, r->width * 3,
                  LCR_PIXEL_RGB8};
  return img;
}

static void json_round_trip(void) {
  const char *json = "{\"name\":\"Fred\",\"loc\":\"45.00,55.0\"}";
  LcrEncodeOptions opts = lcr_encode_options_default();
  LcrRendered sym;
  CHECK(lcr_encode_json(json, strlen(json), &opts, 12, &sym) == LCR_OK);
  CHECK(sym.pixels.len == (size_t)sym.width * sym.height * 3);

  LcrImage img = image_of(&sym);
  LcrDecodeResult res;
  CHECK(lcr_decode(&img, &res) == LCR_OK);
  CHECK(res.kind == LCR_PACKET_UNSIGNED);
  CHECK(res.ctype == LCR_CONTENT_CBOR);
  CHECK(res.diagnostics.version == sym.version);
  CHECK(res.diagnostics.profile == LCR_PROFILE_C1);
  CHECK(res.diagnostics.snr_db > 0.0f);

  char *back = NULL;
  CHECK(lcr_cbor_to_json(res.body.data, res.body.len, &back) == LCR_OK);
  CHECK(back && strcmp(back, json) == 0);
  printf("decoded v%u %ux%u px, SNR %.1f dB: %s\n", sym.version, sym.width, sym.height,
         res.diagnostics.snr_db, back ? back : "(null)");

  LcrProbeResult probe;
  CHECK(lcr_probe(&img, &probe) == LCR_OK);
  CHECK(probe.version == sym.version);

  LcrAccumulator *acc = lcr_accumulator_new();
  CHECK(lcr_accumulator_push(acc, &img) == LCR_OK);
  CHECK(lcr_accumulator_push(acc, &img) == LCR_OK);
  CHECK(lcr_accumulator_frames(acc) == 2);
  LcrDecodeResult combined;
  CHECK(lcr_accumulator_try_decode(acc, &combined) == LCR_OK);
  CHECK(combined.diagnostics.frames_combined == 2);
  lcr_decode_result_free(&combined);
  lcr_accumulator_free(acc);

  lcr_string_free(back);
  lcr_decode_result_free(&res);
  lcr_rendered_free(&sym);
}

static void errors_are_reported(void) {
  LcrRendered sym;
  LcrEncodeOptions opts = lcr_encode_options_default();
  opts.profile = 42;
  CHECK(lcr_encode_text("x", 1, &opts, 12, &sym) == LCR_ERR_ARGUMENT);
  CHECK(lcr_last_error() != NULL && strstr(lcr_last_error(), "profile") != NULL);

  unsigned char grey[32 * 32 * 3];
  memset(grey, 128, sizeof grey);
  LcrImage img = {grey, sizeof grey, 32, 32, 32 * 3, LCR_PIXEL_RGB8};
  LcrDecodeResult res;
  CHECK(lcr_decode(&img, &res) == LCR_ERR_FAILED);
}

static void file_transfer(void) {
  unsigned char file[3000];
  unsigned int s = 0x9e3779b9u;
  for (size_t i = 0; i < sizeof file; i++) {
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    file[i] = (unsigned char)s;
  }
  const unsigned char id[8] = {1, 2, 3, 4, 5, 6, 7, 8};

  uint32_t cap = 0;
  CHECK(lcr_capacity(LCR_PROFILE_C1, 16, 1, 1, true, &cap) == LCR_OK);
  LcrCftTransfer *t = NULL;
  CHECK(lcr_cft_split(file, sizeof file, "noise.bin", id, sizeof id, cap - 32, &t) == LCR_OK);
  size_t n = lcr_cft_transfer_message_count(t);
  CHECK(n > 1);

  LcrEncodeOptions opts = lcr_encode_options_default();
  opts.version = 16;
  LcrCftReceiver *rx = lcr_cft_receiver_new();
  LcrCftStatus st = lcr_cft_receiver_status(rx);
  for (size_t i = 0; i < n; i++) {
    const uint8_t *msg;
    size_t len;
    CHECK(lcr_cft_transfer_message(t, i, &msg, &len) == LCR_OK);
    LcrRendered sym;
    CHECK(lcr_encode_bytes(msg, len, &opts, 8, &sym) == LCR_OK);
    LcrImage img = image_of(&sym);
    LcrDecodeResult res;
    CHECK(lcr_decode(&img, &res) == LCR_OK);
    CHECK(lcr_cft_receiver_accept(rx, res.body.data, res.body.len, &st) == LCR_OK);
    lcr_decode_result_free(&res);
    lcr_rendered_free(&sym);
  }
  CHECK(st.state == LCR_CFT_COMPLETE);
  CHECK(lcr_cft_receiver_missing(rx, NULL, 0) == 0);

  char *name = lcr_cft_receiver_filename(rx);
  CHECK(name && strcmp(name, "noise.bin") == 0);
  LcrBytes got;
  CHECK(lcr_cft_receiver_finish(rx, &got) == LCR_OK);
  CHECK(got.len == sizeof file && memcmp(got.data, file, sizeof file) == 0);
  printf("transferred %zu bytes as %zu symbols, as %s\n", got.len, n, name ? name : "(null)");

  lcr_bytes_free(&got);
  lcr_string_free(name);
  lcr_cft_receiver_free(rx);
  lcr_cft_transfer_free(t);
}

#if defined(LCR_FOUNTAIN)
/* A fountain (CFT.md §21): show frames in order, lose every third, and the receiver still
 * finishes — from whichever symbols it caught, with no request to show anything again. */
static void fountain_transfer(void) {
  unsigned char file[6000];
  unsigned int s = 0x2545f491u;
  for (size_t i = 0; i < sizeof file; i++) {
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    file[i] = (unsigned char)s;
  }
  const unsigned char id[4] = {9, 8, 7, 6};

  LcrCftFountain *f = NULL;
  CHECK(lcr_cft_fountain_new(file, sizeof file, "noise.bin", id, sizeof id, 300, &f) == LCR_OK);
  uint32_t k = lcr_cft_fountain_source_count(f);
  CHECK(k > 10);

  LcrCftReceiver *rx = lcr_cft_receiver_new();
  LcrCftStatus st = lcr_cft_receiver_status(rx);
  uint64_t n = 0;
  for (; n < 20u * k && st.state != LCR_CFT_COMPLETE; n++) {
    if (n % 3 == 2) continue;
    LcrBytes msg;
    CHECK(lcr_cft_fountain_frame(f, n, &msg) == LCR_OK);
    CHECK(lcr_cft_receiver_accept(rx, msg.data, msg.len, &st) == LCR_OK);
    lcr_bytes_free(&msg);
  }
  CHECK(st.state == LCR_CFT_COMPLETE);

  LcrBytes got;
  CHECK(lcr_cft_receiver_finish(rx, &got) == LCR_OK);
  CHECK(got.len == sizeof file && memcmp(got.data, file, sizeof file) == 0);
  printf("fountain: %u chunks, complete after %llu frames with a third lost\n", k,
         (unsigned long long)n);

  lcr_bytes_free(&got);
  lcr_cft_receiver_free(rx);
  lcr_cft_fountain_free(f);
}
#endif

int main(void) {
  printf("liblcr %s\n", lcr_version());
  json_round_trip();
  errors_are_reported();
  file_transfer();
#if defined(LCR_FOUNTAIN)
  fountain_transfer();
#endif
  if (failures) {
    fprintf(stderr, "%d check(s) failed\n", failures);
    return 1;
  }
  printf("ok\n");
  return 0;
}
