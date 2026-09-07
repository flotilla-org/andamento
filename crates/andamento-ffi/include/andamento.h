#ifndef ANDAMENTO_H
#define ANDAMENTO_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

typedef struct Andamento Andamento;

uint32_t andamento_abi_version(void);
/* Input buffers are borrowed only for the duration of a call.
 * Returns NULL on error and writes an owned string to error_out, when supplied.
 * One handle represents one presentation client. Serialize calls per handle.
 */
Andamento *andamento_create(const uint8_t *config_kdl, size_t len, char **error_out);
/* Returns owned UTF-8 JSON, NUL-terminated. See docs/sidebar-design/core-interface.md.
 * Request values are separate from the producer metadata-patch wire protocol.
 * Host effects must be executed locally and completed, including failures.
 */
char *andamento_request(Andamento *, const uint8_t *request_json, size_t len);
void andamento_string_free(char *);
void andamento_destroy(Andamento *);

#ifdef __cplusplus
}
#endif
#endif
