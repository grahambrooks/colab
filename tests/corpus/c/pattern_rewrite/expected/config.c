#include <string.h>
#include "config.h"

void config_set_name(struct config *cfg, const char *name) {
    strlcpy(cfg->name, name, sizeof(cfg->name));
}

void config_reset(struct config *cfg) {
    strlcpy(cfg->name, "default", sizeof(cfg->name));
    cfg->flags = 0;
}
