"""
omen_tcnn.py -- the temporal CNN encoder from OMEN (Claudi & Fiete, MIT),
reproduced for the body-schema model "inspired by OMEN (Claudi & Fiete)".

Source: "MIT Fiete Lab/OMEN_rerun/OMEN-main/omen/modules.py": `make_mlp`,
`ConvLayer`, `tCNN`. The code below is theirs, unchanged except that the
loguru device warning at import is dropped. Only the encoder is reused;
OMEN's CANs (fixed ring/line attractors that would impose topology) are NOT
part of the body-schema model, and are used only afterwards as a ring-vs-
line model-comparison check.

Licence: the OMEN repository carries no LICENSE file. This copy is for
internal research; the authors' terms must be confirmed before any release.
"""
from torch import nn, Tensor
import torch.nn.functional as F

ACTIVATIONS = {"relu": nn.ReLU, "tanh": nn.Tanh, "sigmoid": nn.Sigmoid, "leaky_relu": nn.LeakyReLU}


def make_mlp(n_inputs, n_hidden, n_layers, n_outputs, activation, dropout=0.2):
    activation = ACTIVATIONS[activation]
    if n_layers >= 0:
        layers = [nn.Linear(n_inputs, n_hidden), activation()]
        for _ in range(n_layers - 1):
            layers.append(nn.Linear(n_hidden, n_hidden))
            layers.append(activation())
            if dropout > 0:
                layers.append(nn.Dropout(dropout))
        layers.append(nn.Linear(n_hidden, n_outputs))
    else:
        layers = [nn.Linear(n_inputs, n_outputs)]
    return nn.Sequential(*layers)


class ConvLayer(nn.Module):
    def __init__(self, n_inputs, n_outputs, kernel_size, groups, dilation=1, dropout=0.25):
        super().__init__()
        self.effective_kernel_size = (kernel_size - 1) * dilation + 1
        self.padding = self.effective_kernel_size - 1
        self.conv = nn.Conv1d(in_channels=n_inputs, out_channels=n_outputs, kernel_size=kernel_size,
                              padding=0, groups=groups, dilation=dilation)
        self.dropout = nn.Dropout(dropout)

    def forward(self, x: Tensor) -> Tensor:
        x = F.pad(x, (self.padding, 0), mode="replicate")
        x = self.conv(x)
        x = F.relu(x)
        x = self.dropout(x)
        return x


class tCNN(nn.Module):
    def __init__(self, n_inputs, n_hidden, n_layers, n_outputs, kernel_size, n_kernels, activation,
                 n_conv_groups, channel_kernel_size=3, dilation=3, dropout=0.2, spatial_conv=False):
        super().__init__()
        if n_conv_groups == -1:
            n_conv_groups = n_inputs
        else:
            while n_inputs % n_conv_groups != 0 and n_conv_groups > 1:
                n_conv_groups -= 1
        self.conv_1 = ConvLayer(n_inputs=n_inputs, n_outputs=n_inputs * n_kernels, kernel_size=kernel_size,
                                groups=n_conv_groups, dilation=dilation, dropout=dropout)
        if spatial_conv:
            self.conv_2 = ConvLayer(n_inputs=(n_inputs * n_kernels), n_outputs=n_hidden, kernel_size=channel_kernel_size,
                                    groups=1, dropout=dropout)
        self.mlp = make_mlp(n_hidden if spatial_conv else n_inputs * n_kernels, n_hidden, n_layers, n_outputs, activation, dropout)
        self.spatial_conv = spatial_conv
        self.layer_norm = nn.LayerNorm(n_hidden if spatial_conv else n_inputs * n_kernels)

    def forward(self, x: Tensor) -> Tensor:
        should_squeeze = len(x.shape) == 2
        if should_squeeze:
            x = x.unsqueeze(0)
        x = x.permute(0, 2, 1)
        x = self.conv_1(x)
        if self.spatial_conv:
            x = self.conv_2(x)
        x = x.permute(0, 2, 1)
        x = self.layer_norm(x)
        x = self.mlp(x)
        if should_squeeze:
            x = x.squeeze(0)
        return x
