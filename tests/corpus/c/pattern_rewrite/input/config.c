#include <string.h>
#include "config.h"

void config_set_name(struct config *cfg, const char *name) {
    debug_trace("set name %s", name);
    strcpy(cfg->name, name);
}

void config_reset(struct config *cfg) {
    strcpy(cfg->name, "default");
    cfg->flags = 0;
}
