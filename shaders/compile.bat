@echo off
REM Compile GLSL shaders to SPIR-V for PrismaRev
REM Requires glslc from Vulkan SDK (https://vulkan.lunarg.com/)

set VK_SDK=D:\VulkanSDK\1.4.321.1
set GLSLC=%VK_SDK%\Bin\glslc.exe

if not exist "%GLSLC%" (
    echo ERROR: glslc not found at %GLSLC%
    echo Install Vulkan SDK or set VK_SDK to the correct path.
    exit /b 1
)

echo Compiling shaders...

"%GLSLC%" mesh.vert -o mesh.vert.spv
if %ERRORLEVEL% neq 0 (
    echo FAILED: mesh.vert
    exit /b %ERRORLEVEL%
)
echo   mesh.vert -^> mesh.vert.spv

"%GLSLC%" mesh.frag -o mesh.frag.spv
if %ERRORLEVEL% neq 0 (
    echo FAILED: mesh.frag
    exit /b %ERRORLEVEL%
)
echo   mesh.frag -^> mesh.frag.spv

echo All shaders compiled successfully.
