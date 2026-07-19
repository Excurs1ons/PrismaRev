// Shadow map depth-only fragment stage.
//
// The shadow pass has zero color attachments (the pipeline is created with
// `color_attachment_count = 0`), so there is nothing to write. We only need
// the depth value produced by the rasterizer from the vertex stage's
// clip-space position. A fragment entry is still required by Vulkan for a
// graphics pipeline, but it emits no output.
[shader("fragment")]
void fragmentMain()
{
}
