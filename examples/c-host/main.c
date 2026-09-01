/*
 * A C host for the LC Code component.
 *
 * This exists to test one claim rather than to be a library: §17.3 argues LC Code will be
 * adopted only if embedding is cheap, and the Rust side of that is already proven by
 * `crates/lc-wasm/tests/component.rs`. Rust is not a fair test though — the component is
 * written in Rust, so a Rust host shares its idioms and could hide an assumption. This
 * links against the wasmtime C API instead, and touches nothing Rust-specific.
 *
 * What it asserts, in order:
 *
 *   1. The component instantiates with a linker containing nothing at all. No WASI, no
 *      clock, no randomness. If this ever fails, D8's central promise is broken.
 *   2. `encode-json` accepts a string and a record, and returns a `rendered` record whose
 *      pixel buffer crosses the boundary intact.
 *   3. `decode` takes those pixels back and returns `unsigned(payload)` carrying CBOR.
 *
 * It is deliberately written against the raw C API rather than a generated binding, which
 * is the pessimistic case: a real C host would use `wit-bindgen`'s C generator and write
 * far less than this.
 *
 * Build and run with `just c-host`.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <wasmtime.h>
#include <wasmtime/component/component.h>
#include <wasmtime/component/func.h>
#include <wasmtime/component/instance.h>
#include <wasmtime/component/linker.h>
#include <wasmtime/component/val.h>

static const char *COMPONENT_PATH = "dist/lc-code.wasm";
static const char *JSON = "{\"name\":\"Fred\",\"loc\":\"45.00,55.0\"}";

static int failures = 0;

static void check(int ok, const char *what) {
  printf("  %s %s\n", ok ? "ok  " : "FAIL", what);
  if (!ok)
    failures++;
}

/* Report and clear a wasmtime error, returning 1 if there was one. */
static int had_error(wasmtime_error_t *err, const char *what) {
  if (err == NULL)
    return 0;
  wasm_name_t msg;
  wasmtime_error_message(err, &msg);
  fprintf(stderr, "  FAIL %s: %.*s\n", what, (int)msg.size, msg.data);
  wasm_byte_vec_delete(&msg);
  wasmtime_error_delete(err);
  failures++;
  return 1;
}

/* Set `out` to a record entry `name = val`. The name is copied; wasmtime owns it after. */
static void entry(wasmtime_component_valrecord_entry_t *out, const char *name,
                  wasmtime_component_val_t val) {
  wasm_name_t n;
  wasm_byte_vec_new(&n, strlen(name), name);
  out->name = n;
  out->val = val;
}

static wasmtime_component_val_t val_u32(uint32_t v) {
  wasmtime_component_val_t x;
  x.kind = WASMTIME_COMPONENT_U32;
  x.of.u32 = v;
  return x;
}

static wasmtime_component_val_t val_u8(uint8_t v) {
  wasmtime_component_val_t x;
  x.kind = WASMTIME_COMPONENT_U8;
  x.of.u8 = v;
  return x;
}

static wasmtime_component_val_t val_bool(bool v) {
  wasmtime_component_val_t x;
  x.kind = WASMTIME_COMPONENT_BOOL;
  x.of.boolean = v;
  return x;
}

static wasmtime_component_val_t val_enum(const char *case_name) {
  wasmtime_component_val_t x;
  x.kind = WASMTIME_COMPONENT_ENUM;
  wasm_byte_vec_new(&x.of.enumeration, strlen(case_name), case_name);
  return x;
}

static wasmtime_component_val_t val_none(void) {
  wasmtime_component_val_t x;
  x.kind = WASMTIME_COMPONENT_OPTION;
  x.of.option = NULL;
  return x;
}

static wasmtime_component_val_t val_string(const char *s) {
  wasmtime_component_val_t x;
  x.kind = WASMTIME_COMPONENT_STRING;
  wasm_byte_vec_new(&x.of.string, strlen(s), s);
  return x;
}

/* The `options` record from wit/lc.wit. */
static wasmtime_component_val_t encode_options(const char *profile) {
  wasmtime_component_val_t x;
  x.kind = WASMTIME_COMPONENT_RECORD;
  wasmtime_component_valrecord_new_uninit(&x.of.record, 7);
  entry(&x.of.record.data[0], "profile", val_enum(profile));
  entry(&x.of.record.data[1], "version", val_none());
  entry(&x.of.record.data[2], "ecc-luma", val_u8(1));
  entry(&x.of.record.data[3], "ecc-chroma", val_u8(1));
  entry(&x.of.record.data[4], "compression", val_none());
  entry(&x.of.record.data[5], "reference-border", val_bool(true));
  entry(&x.of.record.data[6], "mask", val_none());
  return x;
}

/* The `image` record from wit/lc.wit. */
static wasmtime_component_val_t image_of(const uint8_t *px, size_t len, uint32_t w,
                                         uint32_t h, uint32_t stride, const char *fmt) {
  wasmtime_component_val_t pixels;
  pixels.kind = WASMTIME_COMPONENT_LIST;
  wasmtime_component_vallist_new_uninit(&pixels.of.list, len);
  for (size_t i = 0; i < len; i++)
    pixels.of.list.data[i] = val_u8(px[i]);

  wasmtime_component_val_t x;
  x.kind = WASMTIME_COMPONENT_RECORD;
  wasmtime_component_valrecord_new_uninit(&x.of.record, 5);
  entry(&x.of.record.data[0], "pixels", pixels);
  entry(&x.of.record.data[1], "width", val_u32(w));
  entry(&x.of.record.data[2], "height", val_u32(h));
  entry(&x.of.record.data[3], "stride", val_u32(stride));
  entry(&x.of.record.data[4], "format", val_enum(fmt));
  return x;
}

/* Field `name` of a record, or NULL. */
static const wasmtime_component_val_t *field(const wasmtime_component_val_t *rec,
                                             const char *name) {
  if (rec->kind != WASMTIME_COMPONENT_RECORD)
    return NULL;
  for (size_t i = 0; i < rec->of.record.size; i++) {
    wasmtime_component_valrecord_entry_t *e = &rec->of.record.data[i];
    if (e->name.size == strlen(name) && memcmp(e->name.data, name, e->name.size) == 0)
      return &e->val;
  }
  return NULL;
}

/* Unwrap `result<T, string>`, printing the error if it is one. */
static const wasmtime_component_val_t *unwrap(const wasmtime_component_val_t *v,
                                              const char *what) {
  if (v->kind != WASMTIME_COMPONENT_RESULT) {
    fprintf(stderr, "  FAIL %s: not a result\n", what);
    failures++;
    return NULL;
  }
  if (!v->of.result.is_ok) {
    const wasmtime_component_val_t *e = v->of.result.val;
    if (e && e->kind == WASMTIME_COMPONENT_STRING)
      fprintf(stderr, "  FAIL %s: %.*s\n", what, (int)e->of.string.size, e->of.string.data);
    else
      fprintf(stderr, "  FAIL %s: err\n", what);
    failures++;
    return NULL;
  }
  return v->of.result.val;
}

/* Look up `name` inside instance export `parent` and get the callable func. */
static bool lookup(wasmtime_component_t *component,
                   wasmtime_component_instance_t *instance, wasmtime_context_t *ctx,
                   wasmtime_component_export_index_t *parent, const char *name,
                   wasmtime_component_func_t *out) {
  wasmtime_component_export_index_t *idx =
      wasmtime_component_get_export_index(component, parent, name, strlen(name));
  if (idx == NULL) {
    fprintf(stderr, "  FAIL no export %s\n", name);
    failures++;
    return false;
  }
  bool ok = wasmtime_component_instance_get_func(instance, ctx, idx, out);
  wasmtime_component_export_index_delete(idx);
  if (!ok) {
    fprintf(stderr, "  FAIL %s is not a function\n", name);
    failures++;
  }
  return ok;
}

int main(void) {
  printf("LC Code — C host trial (wasmtime C API)\n\n");

  /* --- load */
  FILE *f = fopen(COMPONENT_PATH, "rb");
  if (f == NULL) {
    fprintf(stderr, "cannot open %s — run `just component` first\n", COMPONENT_PATH);
    return 1;
  }
  fseek(f, 0, SEEK_END);
  long len = ftell(f);
  fseek(f, 0, SEEK_SET);
  uint8_t *buf = malloc(len);
  if (fread(buf, 1, len, f) != (size_t)len) {
    fprintf(stderr, "short read\n");
    return 1;
  }
  fclose(f);

  wasm_config_t *config = wasm_config_new();
  wasmtime_config_wasm_component_model_set(config, true);
  wasm_engine_t *engine = wasm_engine_new_with_config(config);

  wasmtime_component_t *component = NULL;
  if (had_error(wasmtime_component_new(engine, buf, len, &component), "parse component"))
    return 1;
  free(buf);
  printf("  ok   parsed %s (%ld bytes)\n", COMPONENT_PATH, len);

  /* --- 1. instantiate with a linker that provides nothing whatsoever */
  wasmtime_store_t *store = wasmtime_store_new(engine, NULL, NULL);
  wasmtime_context_t *ctx = wasmtime_store_context(store);
  wasmtime_component_linker_t *linker = wasmtime_component_linker_new(engine);

  wasmtime_component_instance_t instance;
  if (had_error(wasmtime_component_linker_instantiate(linker, ctx, component, &instance),
                "instantiate with an empty linker"))
    return 1;
  check(1, "instantiates with an empty linker — no WASI, no host imports");

  /* --- 2. encode-json */
  wasmtime_component_export_index_t *enc_iface = wasmtime_component_get_export_index(
      component, NULL, "lc:code/encoder@0.4.0", strlen("lc:code/encoder@0.4.0"));
  check(enc_iface != NULL, "exports lc:code/encoder@0.4.0");

  wasmtime_component_func_t encode_json;
  if (!lookup(component, &instance, ctx, enc_iface, "encode-json", &encode_json))
    return 1;

  wasmtime_component_val_t args[3];
  args[0] = val_string(JSON);
  args[1] = encode_options("c1");
  args[2] = val_u32(10);
  wasmtime_component_val_t enc_out = {0};

  if (had_error(wasmtime_component_func_call(&encode_json, ctx, args, 3, &enc_out, 1),
                "encode-json"))
    return 1;
  if (had_error(wasmtime_component_func_post_return(&encode_json, ctx), "post_return"))
    return 1;

  const wasmtime_component_val_t *rendered = unwrap(&enc_out, "encode-json");
  if (rendered == NULL)
    return 1;

  const wasmtime_component_val_t *px = field(rendered, "pixels");
  const wasmtime_component_val_t *w = field(rendered, "width");
  const wasmtime_component_val_t *h = field(rendered, "height");
  const wasmtime_component_val_t *ver = field(rendered, "version");
  check(px && w && h && ver, "rendered record has pixels/width/height/version");
  check(ver->of.u8 == 2, "the §6.1 worked example is a v2 symbol");
  check(px->of.list.size == (size_t)(w->of.u32 * h->of.u32 * 3),
        "pixel buffer is width * height * 3");

  /* Flatten the list back to bytes for the return trip. */
  size_t n = px->of.list.size;
  uint8_t *pixels = malloc(n);
  for (size_t i = 0; i < n; i++)
    pixels[i] = px->of.list.data[i].of.u8;
  uint32_t width = w->of.u32, height = h->of.u32;

  /* --- 3. decode what we just encoded */
  wasmtime_component_export_index_t *dec_iface = wasmtime_component_get_export_index(
      component, NULL, "lc:code/decoder@0.4.0", strlen("lc:code/decoder@0.4.0"));
  check(dec_iface != NULL, "exports lc:code/decoder@0.4.0");

  wasmtime_component_func_t decode;
  if (!lookup(component, &instance, ctx, dec_iface, "decode", &decode))
    return 1;

  wasmtime_component_val_t dec_args[1];
  dec_args[0] = image_of(pixels, n, width, height, width * 3, "rgb8");
  wasmtime_component_val_t dec_out = {0};

  if (had_error(wasmtime_component_func_call(&decode, ctx, dec_args, 1, &dec_out, 1),
                "decode"))
    return 1;
  if (had_error(wasmtime_component_func_post_return(&decode, ctx), "post_return"))
    return 1;

  const wasmtime_component_val_t *result = unwrap(&dec_out, "decode");
  if (result == NULL)
    return 1;

  const wasmtime_component_val_t *packet = field(result, "packet");
  check(packet && packet->kind == WASMTIME_COMPONENT_VARIANT, "packet is a variant");
  check(packet->of.variant.discriminant.size == strlen("unsigned") &&
            memcmp(packet->of.variant.discriminant.data, "unsigned", 8) == 0,
        "packet is `unsigned` — a clean capture of an unsigned payload");

  const wasmtime_component_val_t *payload = packet->of.variant.val;
  const wasmtime_component_val_t *ctype = payload ? field(payload, "ctype") : NULL;
  check(ctype && ctype->kind == WASMTIME_COMPONENT_ENUM &&
            ctype->of.enumeration.size == 4 &&
            memcmp(ctype->of.enumeration.data, "cbor", 4) == 0,
        "ctype is `cbor` — §9.4 transmits CBOR, not JSON");

  const wasmtime_component_val_t *body = payload ? field(payload, "body") : NULL;
  check(body && body->of.list.size > 0, "body crossed the boundary non-empty");

  const wasmtime_component_val_t *diag = field(result, "diagnostics");
  const wasmtime_component_val_t *snr = diag ? field(diag, "snr-db") : NULL;
  const wasmtime_component_val_t *chroma = diag ? field(diag, "chroma-recovered") : NULL;
  check(snr && snr->of.f32 > 10.0f, "diagnostics carry a plausible SNR");
  check(chroma && chroma->of.boolean, "chroma layer recovered");

  printf("\n  decoded %zu CBOR bytes, snr %.1f dB\n", body ? body->of.list.size : 0,
         snr ? snr->of.f32 : 0.0);

  free(pixels);
  wasmtime_component_export_index_delete(enc_iface);
  wasmtime_component_export_index_delete(dec_iface);
  wasmtime_component_linker_delete(linker);
  wasmtime_store_delete(store);
  wasmtime_component_delete(component);
  wasm_engine_delete(engine);

  printf("\n%s\n", failures == 0 ? "all checks passed" : "FAILURES");
  return failures == 0 ? 0 : 1;
}
