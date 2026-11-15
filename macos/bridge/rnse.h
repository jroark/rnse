#include <stdint.h>
#include <stddef.h>
// All operations on a session must occur on one serial queue. Returned buffers
// belong to the session and remain valid until its next API call. Copy before use.
typedef struct Session Session;
Session *rnse_create(void);
void rnse_destroy(Session *s);
// UTF-8 arguments. Returns 0 on success, -1 on error; read buffer kind 0.
int32_t rnse_command(Session *s, const char *command, const char *value);
int32_t rnse_step(Session *s);
// 0: result/error UTF-8; 1: RGB frame; 2: interleaved i16 stereo PCM;
// 3: EUC-JP console; 4: UTF-8 status. Audio/log buffers refreshed by step.
const uint8_t *rnse_buffer(Session *s, uint32_t kind, size_t *len);
uint32_t rnse_width(Session *s);
uint32_t rnse_height(Session *s);
double rnse_time(Session *s);
