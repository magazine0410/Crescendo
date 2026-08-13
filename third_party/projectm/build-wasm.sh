#!/usr/bin/env bash
set -euo pipefail

if ! command -v emcmake >/dev/null || ! command -v em++ >/dev/null; then
  echo "Activate an Emscripten SDK first (emcmake and em++ must be on PATH)." >&2
  exit 1
fi

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

tar -xJf "$script_dir/source/projectm-4.1.5.tar.xz" -C "$work_dir"
tar -xJf "$script_dir/source/presets-en-d.tar.xz" -C "$work_dir"
tar -xJf "$script_dir/source/milkdrop-textures.tar.xz" -C "$work_dir"

projectm_source="$work_dir/crescendo-projectm-4.1.5"
build_dir="$work_dir/build"

emcmake cmake -S "$projectm_source" -B "$build_dir" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DENABLE_PLAYLIST=OFF \
  -DENABLE_SYSTEM_PROJECTM_EVAL=OFF \
  -DENABLE_SDL_UI=OFF \
  -DBUILD_TESTING=OFF \
  -DBUILD_DOCS=OFF
cmake --build "$build_dir" --parallel

(
cd "$repo_root/static/projectm"
em++ "$script_dir/wrapper.cpp" "$build_dir/src/libprojectM/libprojectM-4.a" \
  -I"$projectm_source/src/api/include" \
  -I"$build_dir/src/api/include" \
  -O3 --no-entry \
  -sUSE_SDL=2 \
  -sMIN_WEBGL_VERSION=2 -sMAX_WEBGL_VERSION=2 \
  -sFULL_ES2=1 -sFULL_ES3=1 \
  -sALLOW_MEMORY_GROWTH=1 \
  -sNO_DISABLE_EXCEPTION_CATCHING \
  -sMODULARIZE=1 -sEXPORT_ES6=1 -sEXPORT_NAME=createProjectM \
  -sENVIRONMENT=web -sNO_EXIT_RUNTIME=1 -sFORCE_FILESYSTEM=1 \
  -sEXPORTED_FUNCTIONS=_malloc,_free,_crescendo_projectm_last_error,_crescendo_projectm_init,_crescendo_projectm_destroy,_crescendo_projectm_resize,_crescendo_projectm_render,_crescendo_projectm_add_pcm,_crescendo_projectm_load_preset,_crescendo_projectm_refresh_textures \
  -sEXPORTED_RUNTIME_METHODS=FS,UTF8ToString,HEAPU8,HEAP16 \
  --preload-file "$work_dir/crescendo-projectm-textures/textures@/textures/base" \
  --preload-file "$work_dir/crescendo-projectm-en-d/MD Episode 2 - out of Da club into da head/Textures@/textures/en-d" \
  -o projectm.js
)
