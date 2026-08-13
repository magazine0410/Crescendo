#include <cstdint>
#include <string>

#include <SDL.h>
#include <emscripten/emscripten.h>
#include <emscripten/html5_webgl.h>

#include <projectM-4/audio.h>
#include <projectM-4/core.h>
#include <projectM-4/parameters.h>
#include <projectM-4/render_opengl.h>

namespace {
SDL_Window* window = nullptr;
SDL_GLContext context = nullptr;
projectm_handle instance = nullptr;
std::string last_error;

int fail(const char* message) {
    last_error = message ? message : "Unknown projectM error";
    return 0;
}
}

extern "C" {

EMSCRIPTEN_KEEPALIVE const char* crescendo_projectm_last_error() {
    return last_error.c_str();
}

EMSCRIPTEN_KEEPALIVE int crescendo_projectm_init(int width, int height) {
    if (instance) {
        return 1;
    }

    if (SDL_Init(SDL_INIT_VIDEO) != 0) {
        return fail(SDL_GetError());
    }

    SDL_GL_SetAttribute(SDL_GL_CONTEXT_MAJOR_VERSION, 3);
    SDL_GL_SetAttribute(SDL_GL_CONTEXT_MINOR_VERSION, 0);
    SDL_GL_SetAttribute(SDL_GL_CONTEXT_PROFILE_MASK, SDL_GL_CONTEXT_PROFILE_ES);

    window = SDL_CreateWindow(
        "Crescendo",
        SDL_WINDOWPOS_UNDEFINED,
        SDL_WINDOWPOS_UNDEFINED,
        width,
        height,
        SDL_WINDOW_OPENGL | SDL_WINDOW_RESIZABLE | SDL_WINDOW_ALLOW_HIGHDPI
    );
    if (!window) {
        return fail(SDL_GetError());
    }

    context = SDL_GL_CreateContext(window);
    if (!context) {
        return fail(SDL_GetError());
    }
    if (SDL_GL_MakeCurrent(window, context) != 0) {
        return fail(SDL_GetError());
    }

    const auto webgl_context = emscripten_webgl_get_current_context();
    if (!webgl_context) {
        return fail("WebGL2 context creation failed");
    }
    // projectM's Emscripten notes recommend enabling this WebGL 1 extension.
    // WebGL 2 promotes floating-point textures into the core API, and WebKit
    // therefore may correctly return false because there is no extension
    // object to enable. Probe it for browsers that expose it, but do not reject
    // an otherwise valid WebGL 2 context.
    emscripten_webgl_enable_extension(webgl_context, "OES_texture_float");

    instance = projectm_create();
    if (!instance) {
        return fail("projectM could not create its OpenGL renderer");
    }

    const char* texture_paths[] = {"/textures", "/imports"};
    projectm_set_texture_search_paths(instance, texture_paths, 2);
    projectm_set_window_size(instance, width, height);
    projectm_set_mesh_size(instance, 48, 32);
    projectm_set_fps(instance, 60);
    projectm_set_aspect_correction(instance, true);
    projectm_set_hard_cut_enabled(instance, false);
    projectm_set_preset_locked(instance, true);
    last_error.clear();
    return 1;
}

EMSCRIPTEN_KEEPALIVE void crescendo_projectm_destroy() {
    if (instance) {
        projectm_destroy(instance);
        instance = nullptr;
    }
    if (context) {
        SDL_GL_DeleteContext(context);
        context = nullptr;
    }
    if (window) {
        SDL_DestroyWindow(window);
        window = nullptr;
    }
    SDL_QuitSubSystem(SDL_INIT_VIDEO);
}

EMSCRIPTEN_KEEPALIVE void crescendo_projectm_resize(int width, int height) {
    if (!instance || !window || width < 1 || height < 1) {
        return;
    }
    SDL_SetWindowSize(window, width, height);
    projectm_set_window_size(instance, width, height);
}

EMSCRIPTEN_KEEPALIVE void crescendo_projectm_render() {
    if (!instance || !window || !context) {
        return;
    }
    SDL_GL_MakeCurrent(window, context);
    projectm_opengl_render_frame(instance);
}

EMSCRIPTEN_KEEPALIVE void crescendo_projectm_add_pcm(const int16_t* samples, unsigned int frames) {
    if (instance && samples && frames > 0) {
        projectm_pcm_add_int16(instance, samples, frames, PROJECTM_STEREO);
    }
}

EMSCRIPTEN_KEEPALIVE void crescendo_projectm_load_preset(const char* data, int smooth) {
    if (instance && data) {
        projectm_load_preset_data(instance, data, smooth != 0);
    }
}

EMSCRIPTEN_KEEPALIVE void crescendo_projectm_refresh_textures() {
    if (instance) {
        const char* texture_paths[] = {"/textures", "/imports"};
        projectm_set_texture_search_paths(instance, texture_paths, 2);
    }
}

}
