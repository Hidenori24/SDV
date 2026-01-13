static Rte::DriverInput g_driver_input{};
static Rte::Safety g_safety{};
static Rte::ActuatorCmd g_actuator_cmd{};

Rte::DriverInput Mock_Rte_Read_DriverInput()
{
    return g_driver_input;
}

void Mock_Rte_SetDriverInput(const Rte::DriverInput &in)
{
    g_driver_input = in;
}