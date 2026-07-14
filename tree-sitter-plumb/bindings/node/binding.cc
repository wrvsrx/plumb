#include <napi.h>

typedef struct TSLanguage TSLanguage;

extern "C" TSLanguage *tree_sitter_plumb();

Napi::Object Init(Napi::Env env, Napi::Object exports) {
  exports["language"] = Napi::External<TSLanguage>::New(env, tree_sitter_plumb());
  return exports;
}

NODE_API_MODULE(tree_sitter_plumb_binding, Init)
