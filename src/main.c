//! Test the ffi interface from C.

#include <stdio.h>

#include "ppoprf.h"

int test_create() {
  printf("Testing server create/release:\n");
  printf("  creating server...\n");
  RandomnessServer* server = randomness_server_create();
  if (server == NULL) {
    return -1;
  }
  printf("  releasing server...\n");
  randomness_server_release(server);
  return 0;
}

int main() {
  int success = 0;
  int failure = 0;

  int r = test_create();
  if (r) {
    printf("Test FAILED\n");
    failure++;
  } else {
    success++;
  }

  printf("%d/%d tests successful\n", success, success+failure);
  if (failure) {
    printf("%d tests FAILED!", failure);
    return failure;
  }

  return 0;
}
