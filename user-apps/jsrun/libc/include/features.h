/* Kitsune jsrun libc shim: musl's internal <features.h> (the libm sources use `hidden` and `weak_alias`). */
#ifndef KJS_FEATURES_H
#define KJS_FEATURES_H
#define hidden __attribute__((__visibility__("hidden")))
#define weak_alias(old, new) extern __typeof(old) new __attribute__((__weak__, __alias__(#old)))
#endif
