@echo off
setlocal
rem Only Cargo owns building and locating the executable. Never bypass Agent protection.
pushd "%~dp0..\tomcat" || exit /b 1
call cargo run --release --bin tomcat -- self-install
set "result=%errorlevel%"
popd
exit /b %result%
