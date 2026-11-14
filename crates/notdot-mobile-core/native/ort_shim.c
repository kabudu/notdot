/*
 * A narrow C shim over the ONNX Runtime C API, compiled against the pinned
 * official header (native/onnxruntime, see README.md) and linked to the
 * runtime sherpa-onnx already ships (OrtGetApiBase). Function access goes
 * through the header's typed OrtApi struct; nothing is numbered by hand.
 *
 * Exposes three things to Rust: open a CPU session from a model file, run
 * it with typed inputs and copy out one float output, and free. Every input
 * and output is validated here (element type, rank, non-negative
 * dimensions, overflow-checked element counts, byte lengths, an output size
 * cap), and every runtime object is released on every path. No callbacks
 * into Rust, so no Rust panic can unwind through C; the ONNX Runtime C API
 * catches its own C++ exceptions and returns them as OrtStatus errors.
 * Input buffers are only read (ONNX Runtime does not write to inputs).
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "onnxruntime_c_api.h"

/* Every function used here exists in this API version (ONNX Runtime 1.16). */
#define ND_ORT_API_VERSION 16
#define ND_MAX_RANK 8
#define ND_MAX_INPUTS 64

enum nd_type { ND_F32 = 1, ND_I64 = 2, ND_BOOL = 3 };

typedef struct nd_session {
    const OrtApi *api;
    OrtEnv *env;
    OrtSession *session;
    OrtMemoryInfo *memory;
} nd_session;

static void put_error(char *err, size_t len, const char *message) {
    if (err != NULL && len > 0) {
        snprintf(err, len, "%s", message != NULL ? message : "unknown error");
    }
}

/* Take a status: 0 if OK, else copy its message, release it, return -1. */
static int check(const OrtApi *api, OrtStatus *status, char *err, size_t len) {
    if (status == NULL) {
        return 0;
    }
    put_error(err, len, api->GetErrorMessage(status));
    api->ReleaseStatus(status);
    return -1;
}

static const OrtApi *get_api(char *err, size_t len) {
    const OrtApiBase *base = OrtGetApiBase();
    const OrtApi *api = base != NULL ? base->GetApi(ND_ORT_API_VERSION) : NULL;
    if (api == NULL) {
        put_error(err, len, "the bundled ONNX Runtime is too old");
    }
    return api;
}

void nd_ort_close(nd_session *s) {
    if (s == NULL) {
        return;
    }
    if (s->memory != NULL) {
        s->api->ReleaseMemoryInfo(s->memory);
    }
    if (s->session != NULL) {
        s->api->ReleaseSession(s->session);
    }
    if (s->env != NULL) {
        s->api->ReleaseEnv(s->env);
    }
    free(s);
}

nd_session *nd_ort_open(const char *path, int threads, char *err, size_t err_len) {
    const OrtApi *api = get_api(err, err_len);
    if (api == NULL || path == NULL || threads < 1 || threads > 16) {
        if (api != NULL) {
            put_error(err, err_len, "invalid session request");
        }
        return NULL;
    }
    nd_session *s = calloc(1, sizeof(nd_session));
    if (s == NULL) {
        put_error(err, err_len, "out of memory");
        return NULL;
    }
    s->api = api;
    OrtSessionOptions *options = NULL;
    if (check(api, api->CreateEnv(ORT_LOGGING_LEVEL_ERROR, "notdot-wake", &s->env), err, err_len) ||
        check(api, api->CreateSessionOptions(&options), err, err_len)) {
        goto fail;
    }
    if (check(api, api->SetSessionGraphOptimizationLevel(options, ORT_ENABLE_ALL), err, err_len) ||
        check(api, api->SetIntraOpNumThreads(options, threads), err, err_len) ||
        check(api, api->SetInterOpNumThreads(options, 1), err, err_len) ||
        check(api, api->CreateSession(s->env, path, options, &s->session), err, err_len) ||
        check(api, api->CreateCpuMemoryInfo(OrtArenaAllocator, OrtMemTypeDefault, &s->memory), err,
              err_len)) {
        goto fail;
    }
    api->ReleaseSessionOptions(options);
    return s;
fail:
    if (options != NULL) {
        api->ReleaseSessionOptions(options);
    }
    nd_ort_close(s);
    return NULL;
}

/* Overflow-checked element count of a shape; -1 if invalid. */
static int count_elements(const int64_t *shape, size_t rank, size_t *out) {
    if (rank > ND_MAX_RANK || (rank > 0 && shape == NULL)) {
        return -1;
    }
    size_t n = 1;
    for (size_t i = 0; i < rank; i++) {
        if (shape[i] < 0) {
            return -1;
        }
        size_t d = (size_t)shape[i];
        if (d != 0 && n > SIZE_MAX / d) {
            return -1;
        }
        n *= d;
    }
    *out = n;
    return 0;
}

/*
 * Run with `n_in` inputs and copy out the float output `output_name`.
 * On success *out_data is a malloc'd buffer of *out_len floats (free with
 * nd_ort_free; NULL when empty) and out_shape[0..*out_rank] its shape.
 * Returns 0 on success, -1 on error (message in err).
 */
int nd_ort_run(nd_session *s, size_t n_in, const char *const *names, const int *types,
               const void *const *data, const size_t *bytes, const int64_t *const *shapes,
               const size_t *ranks, const char *output_name, size_t max_out, float **out_data,
               size_t *out_len, int64_t *out_shape, size_t *out_rank, char *err, size_t err_len) {
    static const int64_t empty_storage = 0;
    if (s == NULL || out_data == NULL || out_len == NULL || out_shape == NULL || out_rank == NULL ||
        output_name == NULL || n_in == 0 || n_in > ND_MAX_INPUTS || names == NULL ||
        types == NULL || data == NULL || bytes == NULL || shapes == NULL || ranks == NULL) {
        put_error(err, err_len, "invalid run request");
        return -1;
    }
    const OrtApi *api = s->api;
    *out_data = NULL;
    *out_len = 0;
    *out_rank = 0;
    OrtValue *inputs[ND_MAX_INPUTS] = {0};
    OrtValue *output = NULL;
    OrtTensorTypeAndShapeInfo *info = NULL;
    int result = -1;
    for (size_t i = 0; i < n_in; i++) {
        size_t elements = 0, size = 0;
        ONNXTensorElementDataType kind;
        switch (types[i]) {
        case ND_F32: kind = ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT; size = sizeof(float); break;
        case ND_I64: kind = ONNX_TENSOR_ELEMENT_DATA_TYPE_INT64; size = sizeof(int64_t); break;
        case ND_BOOL: kind = ONNX_TENSOR_ELEMENT_DATA_TYPE_BOOL; size = 1; break;
        default: put_error(err, err_len, "unsupported input type"); goto done;
        }
        if (names[i] == NULL || count_elements(shapes[i], ranks[i], &elements) != 0 ||
            (elements != 0 && elements > SIZE_MAX / size) || elements * size != bytes[i] ||
            (bytes[i] != 0 && data[i] == NULL)) {
            put_error(err, err_len, "input size does not match its shape");
            goto done;
        }
        /* Empty tensors still need a valid, aligned data pointer. */
        void *p = bytes[i] == 0 ? (void *)&empty_storage : (void *)data[i];
        if (check(api,
                  api->CreateTensorWithDataAsOrtValue(s->memory, p, bytes[i], shapes[i], ranks[i],
                                                      kind, &inputs[i]),
                  err, err_len)) {
            goto done;
        }
    }
    if (check(api,
              api->Run(s->session, NULL, names, (const OrtValue *const *)inputs, n_in,
                       &output_name, 1, &output),
              err, err_len)) {
        goto done;
    }
    if (check(api, api->GetTensorTypeAndShape(output, &info), err, err_len)) {
        goto done;
    }
    ONNXTensorElementDataType out_kind;
    size_t rank = 0;
    if (check(api, api->GetTensorElementType(info, &out_kind), err, err_len) ||
        check(api, api->GetDimensionsCount(info, &rank), err, err_len)) {
        goto done;
    }
    if (out_kind != ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT || rank > ND_MAX_RANK) {
        put_error(err, err_len, "unexpected output type or rank");
        goto done;
    }
    if (check(api, api->GetDimensions(info, out_shape, rank), err, err_len)) {
        goto done;
    }
    size_t elements = 0;
    if (count_elements(out_shape, rank, &elements) != 0 || elements > max_out ||
        elements > SIZE_MAX / sizeof(float)) {
        put_error(err, err_len, "unexpected output size");
        goto done;
    }
    if (elements > 0) {
        float *source = NULL;
        if (check(api, api->GetTensorMutableData(output, (void **)&source), err, err_len)) {
            goto done;
        }
        if (source == NULL) {
            put_error(err, err_len, "missing output data");
            goto done;
        }
        float *copy = malloc(elements * sizeof(float));
        if (copy == NULL) {
            put_error(err, err_len, "out of memory");
            goto done;
        }
        memcpy(copy, source, elements * sizeof(float));
        *out_data = copy;
    }
    *out_len = elements;
    *out_rank = rank;
    result = 0;
done:
    if (info != NULL) {
        api->ReleaseTensorTypeAndShapeInfo(info);
    }
    if (output != NULL) {
        api->ReleaseValue(output);
    }
    for (size_t i = 0; i < n_in; i++) {
        if (inputs[i] != NULL) {
            api->ReleaseValue(inputs[i]);
        }
    }
    return result;
}

void nd_ort_free(float *data) { free(data); }

/* ---- Introspection and multi-output runs (streaming encoders) ---- */

#define ND_MAX_OUTPUTS 64

/* Number of inputs (outputs != 0: outputs); 0 on error. */
size_t nd_ort_count(nd_session *s, int outputs) {
    if (s == NULL) {
        return 0;
    }
    size_t n = 0;
    OrtStatus *status = outputs ? s->api->SessionGetOutputCount(s->session, &n)
                                : s->api->SessionGetInputCount(s->session, &n);
    if (status != NULL) {
        s->api->ReleaseStatus(status);
        return 0;
    }
    return n;
}

static int map_type(ONNXTensorElementDataType kind) {
    switch (kind) {
    case ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT: return ND_F32;
    case ONNX_TENSOR_ELEMENT_DATA_TYPE_INT64: return ND_I64;
    case ONNX_TENSOR_ELEMENT_DATA_TYPE_BOOL: return ND_BOOL;
    default: return 0;
    }
}

/*
 * Describe input (or output) i: its name (NUL-terminated, at most
 * name_len - 1 bytes; longer names are an error), element type (ND_*, 0 if
 * unsupported) and shape (-1 for a dynamic dimension, at most ND_MAX_RANK).
 */
int nd_ort_describe(nd_session *s, int outputs, size_t i, char *name, size_t name_len, int *type,
                    int64_t *dims, size_t *rank, char *err, size_t err_len) {
    if (s == NULL || name == NULL || name_len == 0 || type == NULL || dims == NULL || rank == NULL) {
        put_error(err, err_len, "invalid describe request");
        return -1;
    }
    const OrtApi *api = s->api;
    OrtAllocator *allocator = NULL;
    char *raw = NULL;
    OrtTypeInfo *info = NULL;
    int result = -1;
    if (check(api, api->GetAllocatorWithDefaultOptions(&allocator), err, err_len)) {
        goto done;
    }
    if (check(api,
              outputs ? api->SessionGetOutputName(s->session, i, allocator, &raw)
                      : api->SessionGetInputName(s->session, i, allocator, &raw),
              err, err_len)) {
        goto done;
    }
    if (raw == NULL || strlen(raw) >= name_len) {
        put_error(err, err_len, "model name too long");
        goto done;
    }
    memcpy(name, raw, strlen(raw) + 1);
    if (check(api,
              outputs ? api->SessionGetOutputTypeInfo(s->session, i, &info)
                      : api->SessionGetInputTypeInfo(s->session, i, &info),
              err, err_len)) {
        goto done;
    }
    const OrtTensorTypeAndShapeInfo *tensor = NULL; /* owned by info */
    if (check(api, api->CastTypeInfoToTensorInfo(info, &tensor), err, err_len)) {
        goto done;
    }
    if (tensor == NULL) {
        put_error(err, err_len, "not a tensor");
        goto done;
    }
    ONNXTensorElementDataType kind;
    size_t n = 0;
    if (check(api, api->GetTensorElementType(tensor, &kind), err, err_len) ||
        check(api, api->GetDimensionsCount(tensor, &n), err, err_len)) {
        goto done;
    }
    if (n > ND_MAX_RANK) {
        put_error(err, err_len, "rank too large");
        goto done;
    }
    if (check(api, api->GetDimensions(tensor, dims, n), err, err_len)) {
        goto done;
    }
    *type = map_type(kind);
    *rank = n;
    result = 0;
done:
    if (info != NULL) {
        api->ReleaseTypeInfo(info);
    }
    if (raw != NULL && allocator != NULL) {
        (void)allocator->Free(allocator, raw);
    }
    return result;
}

/*
 * Run with n_in validated inputs and copy out n_out named outputs, each
 * float or int64, each at most max_out elements and together at most
 * max_total (checked before each copy is allocated). On success out_data[k] is
 * a malloc'd buffer (free with nd_ort_free_any; NULL when empty) of
 * out_len[k] elements of type out_types[k], with shape
 * out_shapes[k * ND_MAX_RANK ..][0..out_ranks[k]]. On failure nothing is
 * left allocated. Returns 0 on success, -1 on error.
 */
int nd_ort_run_many(nd_session *s, size_t n_in, const char *const *names, const int *types,
                    const void *const *data, const size_t *bytes, const int64_t *const *shapes,
                    const size_t *ranks, size_t n_out, const char *const *out_names, size_t max_out,
                    size_t max_total, int *out_types, void **out_data, size_t *out_len,
                    int64_t *out_shapes, size_t *out_ranks, char *err, size_t err_len) {
    static const int64_t empty_storage = 0;
    size_t total = 0; /* elements copied so far, bounded by max_total */
    if (s == NULL || n_in == 0 || n_in > ND_MAX_INPUTS || names == NULL || types == NULL ||
        data == NULL || bytes == NULL || shapes == NULL || ranks == NULL || n_out == 0 ||
        n_out > ND_MAX_OUTPUTS || out_names == NULL || out_types == NULL || out_data == NULL ||
        out_len == NULL || out_shapes == NULL || out_ranks == NULL) {
        put_error(err, err_len, "invalid run request");
        return -1;
    }
    const OrtApi *api = s->api;
    for (size_t k = 0; k < n_out; k++) {
        out_data[k] = NULL;
        out_len[k] = 0;
        out_ranks[k] = 0;
        out_types[k] = 0;
        if (out_names[k] == NULL) {
            put_error(err, err_len, "invalid output name");
            return -1;
        }
    }
    OrtValue *inputs[ND_MAX_INPUTS] = {0};
    OrtValue *outputs[ND_MAX_OUTPUTS] = {0};
    OrtTensorTypeAndShapeInfo *info = NULL;
    int result = -1;
    for (size_t i = 0; i < n_in; i++) {
        size_t elements = 0, size = 0;
        ONNXTensorElementDataType kind;
        switch (types[i]) {
        case ND_F32: kind = ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT; size = sizeof(float); break;
        case ND_I64: kind = ONNX_TENSOR_ELEMENT_DATA_TYPE_INT64; size = sizeof(int64_t); break;
        case ND_BOOL: kind = ONNX_TENSOR_ELEMENT_DATA_TYPE_BOOL; size = 1; break;
        default: put_error(err, err_len, "unsupported input type"); goto done;
        }
        if (names[i] == NULL || count_elements(shapes[i], ranks[i], &elements) != 0 ||
            (elements != 0 && elements > SIZE_MAX / size) || elements * size != bytes[i] ||
            (bytes[i] != 0 && data[i] == NULL)) {
            put_error(err, err_len, "input size does not match its shape");
            goto done;
        }
        void *p = bytes[i] == 0 ? (void *)&empty_storage : (void *)data[i];
        if (check(api,
                  api->CreateTensorWithDataAsOrtValue(s->memory, p, bytes[i], shapes[i], ranks[i],
                                                      kind, &inputs[i]),
                  err, err_len)) {
            goto done;
        }
    }
    if (check(api,
              api->Run(s->session, NULL, names, (const OrtValue *const *)inputs, n_in, out_names,
                       n_out, outputs),
              err, err_len)) {
        goto done;
    }
    for (size_t k = 0; k < n_out; k++) {
        if (outputs[k] == NULL) {
            put_error(err, err_len, "missing output");
            goto done;
        }
        if (check(api, api->GetTensorTypeAndShape(outputs[k], &info), err, err_len)) {
            goto done;
        }
        ONNXTensorElementDataType kind;
        size_t rank = 0;
        if (check(api, api->GetTensorElementType(info, &kind), err, err_len) ||
            check(api, api->GetDimensionsCount(info, &rank), err, err_len)) {
            goto done;
        }
        size_t size = kind == ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT   ? sizeof(float)
                      : kind == ONNX_TENSOR_ELEMENT_DATA_TYPE_INT64 ? sizeof(int64_t)
                                                                    : 0;
        if (size == 0 || rank > ND_MAX_RANK) {
            put_error(err, err_len, "unexpected output type or rank");
            goto done;
        }
        int64_t *shape = out_shapes + k * ND_MAX_RANK;
        if (check(api, api->GetDimensions(info, shape, rank), err, err_len)) {
            goto done;
        }
        api->ReleaseTensorTypeAndShapeInfo(info);
        info = NULL;
        size_t elements = 0;
        if (count_elements(shape, rank, &elements) != 0 || elements > max_out ||
            elements > SIZE_MAX / size || elements > max_total - total) {
            put_error(err, err_len, "unexpected output size");
            goto done;
        }
        total += elements;
        if (elements > 0) {
            void *source = NULL;
            if (check(api, api->GetTensorMutableData(outputs[k], &source), err, err_len)) {
                goto done;
            }
            if (source == NULL) {
                put_error(err, err_len, "missing output data");
                goto done;
            }
            void *copy = malloc(elements * size);
            if (copy == NULL) {
                put_error(err, err_len, "out of memory");
                goto done;
            }
            memcpy(copy, source, elements * size);
            out_data[k] = copy;
        }
        out_types[k] = map_type(kind);
        out_len[k] = elements;
        out_ranks[k] = rank;
    }
    result = 0;
done:
    if (info != NULL) {
        api->ReleaseTensorTypeAndShapeInfo(info);
    }
    for (size_t k = 0; k < n_out; k++) {
        if (outputs[k] != NULL) {
            api->ReleaseValue(outputs[k]);
        }
        if (result != 0) {
            free(out_data[k]);
            out_data[k] = NULL;
            out_len[k] = 0;
        }
    }
    for (size_t i = 0; i < n_in; i++) {
        if (inputs[i] != NULL) {
            api->ReleaseValue(inputs[i]);
        }
    }
    return result;
}

void nd_ort_free_any(void *data) { free(data); }

/* The linked runtime's version string (static). */
const char *nd_ort_version(void) {
    const OrtApiBase *base = OrtGetApiBase();
    return base != NULL ? base->GetVersionString() : "";
}
