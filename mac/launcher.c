// Main executable of the installer app. Notarization needs a signed Mach-O
// here, not a shell script, so this just runs Resources/install-usb.
#include <libgen.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(void) {
  char exe[PATH_MAX], real[PATH_MAX], script[PATH_MAX];
  uint32_t n = sizeof exe;
  if (_NSGetExecutablePath(exe, &n) != 0 || realpath(exe, real) == NULL) return 1;
  snprintf(script, sizeof script, "%s/../Resources/install-usb", dirname(real));
  setenv("GUI", "1", 1);
  execl("/bin/sh", "sh", script, (char *)NULL);
  perror("exec");
  return 1;
}
