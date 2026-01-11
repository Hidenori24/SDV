#pragma once
namespace Swc::VehicleDynamics {
void Init();
void Step10ms(double dt_s);
const char* Version();
}