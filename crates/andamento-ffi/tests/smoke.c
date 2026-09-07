#include "andamento.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

int main(void) {
    const char *config = "region \"header\" source=\"header\" root-template=\"flotilla/region/header\" form=\"compact\"\n";
    char *error = NULL;
    assert(andamento_abi_version() == 1);
    Andamento *sidebar = andamento_create((const uint8_t *)config, strlen(config), &error);
    assert(sidebar != NULL && error == NULL);
    const char *request = "{\"request\":\"snapshot\"}";
    char *response = andamento_request(sidebar, (const uint8_t *)request, strlen(request));
    assert(response != NULL && strstr(response, "\"ok\":true") != NULL);
    assert(strstr(response, "sections") != NULL);
    andamento_string_free(response);
    andamento_destroy(sidebar);
    puts("C ABI snapshot roundtrip passed");
    return 0;
}
